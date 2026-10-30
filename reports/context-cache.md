# Reasoning loops and prompt-cache misses: what was actually wrong

Two complaints, one investigation: the agent loops on the same work, and the
prompt-cache hit rate never reaches 98%. The short version:

* the cache hit rate that karucode *reported* was a false zero (two
  measurement bugs), and
* the cache hit rate it *achieved* was genuinely bad once a run went over the
  context budget (median 1.6% per call on a real looping subagent), because
  compaction rewrote the front of the conversation on every turn;
* the loop itself was ours: the same transcript shows 44 identical tool calls
  repeated 24-29 times while 722 of 807 requests carried **no user message at
  all** - the task had been compacted away and replaced by a digest written in
  the model's own voice.

Everything below is measured, not inferred. Reproduction steps are at the end.

## Method

1. `~/.local/share/karucode/karucode.db` was read read-only. Sessions, runs and
   messages were replayed through the shipped context algorithm
   (`crates/kc-runtime/src/context.rs`) and consecutive requests were diffed to
   get the longest common prefix - the only thing a prefix cache can hit.
2. The gateway (`https://ai.karutoil.site/v1`) was probed directly to learn what
   it reports, and then driven through the real `karucode` binary, with a
   logging proxy in between to see the request and response frames.

## Finding 1 - the reported hit rate was always zero (measurement)

The gateway only reports real cache numbers when the request asks for the usage
block:

| request | `prompt_tokens` | `cached_tokens` |
| --- | --- | --- |
| `stream: true` without `stream_options` | 9,039 | **0** |
| `stream: true` with `stream_options.include_usage` | 9,039 | 8,960 (99.1%) |

`kc-provider` never sent `stream_options`. Worse, it stopped reading the stream at
the chunk that carries `finish_reason`, and OpenAI-compatible endpoints send the
usage block in the chunk *after* it:

```
finish_reason=None  usage=False
finish_reason='length' usage=False     <- old code returned here
finish_reason=None  usage=True         <- the token counts, and the cache counts
[DONE]
```

So token accounting only ever worked against test mocks that emit usage first.
On a real gateway karucode recorded no usage at all, and any dashboard fed by
those records showed a 0% cache hit rate for calls that were 99% cached.

## Finding 2 - the achieved hit rate collapsed over budget (real)

Replaying the stored transcripts through the old algorithm:

| transcript | calls | median hit | calls < 10% | compactions |
| --- | --- | --- | --- | --- |
| `s_efe00221…` (a normal long session) | 198 | 96.3% | 0.5% | 1 |
| `s_ecd6e7…/a_8b3ada…` (the looping subagent) | 806 | **1.6%** | **57.5%** | 792 |

Cause: `compact_run_window` dropped only what barely did not fit. The window grew
by one exchange per turn, so one more exchange was dropped per turn, so the
digest at the front of the conversation was rebuilt **on every turn**. A prefix
cache hits 0% from the first changed byte, and that byte was at position 1.

Live confirmation that this is the whole story (same gateway, 9k-token prompt):

| request | cached |
| --- | --- |
| identical prefix + one appended turn | 99.0% |
| same, with `messages[1]` mutated (what the digest did) | **0.0%** |

## Finding 3 - the loop was ours

From the same subagent transcript (807 model calls):

* 44 distinct tool calls were repeated 24-29 times each - the same five
  `read_file` paths and `git_status`, over and over;
* **722 of 807 requests contained no user message** - the task had been
  compacted away. The only thing that still described the job was the digest,
  which was inserted with `role: assistant`, i.e. the model was reading a summary
  of work "it" had allegedly done, in its own voice;
* 792 of 807 requests carried a digest that had just been rewritten;
* the digest kept ~1% of the dropped text and **0% of tool output**, so the model
  could not remember what it had read even in principle;
* 32 of 807 requests were still sent *over* `context_max_chars` (225k against a
  240k budget in the final one), because `fit` cannot trim a subagent history at
  all: its protection boundary is "the last user message", and a subagent has
  exactly one, at index 0.

A model that is told "you already did this", shown a different summary every
turn, given no instruction, and made to re-ask for files it cannot remember is
doing exactly what the context told it to do.

## What changed

`crates/kc-runtime/src/context.rs`

* Compaction is quantised: it triggers at `context_max_chars` and cuts down to
  `max(15% of budget, budget - 50 turns of headroom)`. One cache miss per
  compaction instead of one per turn.
* The first user turn (the task) is pinned and never compacted.
* The exchange in flight is never compacted (the newest assistant turn with tool
  calls and every result answering it).
* The digest is a **user** turn, states that the work is done and must not be
  redone, names each tool call with its arguments (`read_file({"path":…})`), and
  rolls forward into the next digest instead of nesting.
* `fit` now has a real fallback boundary when there is no "last user turn", so an
  over-budget window is never sent as-is.

`crates/kc-runtime/src/agent.rs`

* A turn's tool output is capped at half the context budget, spread over the
  calls of that turn: a 32-call turn at the 24 KB per-call cap used to be 768 KB
  of tool output, three budgets in one request.
* A tool result that repeats an identical answer gets a note after the third
  time (`"this exact call has now returned an identical result 3 times"`), which
  is persisted with the result. It warns; it does not block a legitimate re-read.
* The provider stream is read past `finish_reason` (bounded by a 3s window) so
  the trailing usage block is not thrown away.
* `cached_tokens` is logged per call (`prompt cache input_tokens=… cached_tokens=…
  cache_hit_percent=…`).

`crates/kc-provider`

* `stream_options: {"include_usage": true}` is sent (overridable through
  `extra_body` for gateways that reject it).
* `cached_tokens` is parsed from all three shapes in the wild
  (`prompt_tokens_details.cached_tokens`, `prompt_cache_hit_tokens`,
  `cache_read_input_tokens`).

`crates/kc-core`, `web/`

* `Event::Usage` carries `cached_tokens` (omitted when the endpoint does not
  report it - "unknown", not 0%), accumulated across the run.
* The chat HUD reads `N in · M out · P% cached`.
* New `@kc-doc` topic `prompt-cache` (`docs/internal/prompt-cache.md`).

## Result

Same transcripts, post-fix policy:

| transcript | `context_max_chars` | median hit | compactions |
| --- | --- | --- | --- |
| looping subagent | 240,000 | 96.3% | 119 (was 792) |
| looping subagent | 1,000,000 | **98.9%** | 11 |
| normal session | 240,000 | 96.3% | 1 |

Live, through the real binary and the real gateway, after the fix:

```
DEBUG prompt cache input_tokens=3265 cached_tokens=3200 cache_hit_percent=98.0
DEBUG prompt cache input_tokens=3282 cached_tokens=3200 cache_hit_percent=97.5
DEBUG prompt cache input_tokens=3299 cached_tokens=3200 cache_hit_percent=97.0
```

## Round 2 - the loop survived the cache fix, and the reason is the empty transcript

A subagent run *after* the compaction fix still looped. Measured on the stored
transcript (133 tool calls, one 2,856-char task, 1.73 MB of tool output):

* **118 `read_file` calls over 18 distinct files** - `agent.rs` 27 times,
  `context.rs` 18, `approvals.rs` 14 - and 68 of them paged a *different*
  `offset/limit` window, which is why it looked like "the same files with
  different reasoning";
* **every assistant turn contained 2 characters of text**. The model streams its
  analysis as `reasoning_content`, which the UI renders and the runtime discards;
  it emits no final prose either, so the transcript holds tool calls and tool
  output and **nothing the model concluded**;
* compaction therefore killed the only memory it had (the tool output), while the
digest could only summarise the 2-character assistant turns.

A read-only review of 1.7 MB against a 240k-char window cannot fit, and the model
had no way to write anything down. The loop was structural, not behavioural.

### What changed now

* **Written findings are sticky** (`context.rs`): an assistant turn with >= 200
  chars of text and no tool calls is never compacted while it fits a note budget
  of `budget/20` (1k-12k chars), oldest released first. Findings are never
  truncated.
* **Silence nudge** (`agent.rs`): after 3 consecutive assistant turns with < 40
  chars of text, the request carries a note saying that conclusions held in
  reasoning are not kept and findings must be written as text.
* **Read ledger** (`context.rs::ReadLedger` + `agent.rs`): every request ends with
  a short note listing the paths already opened, how often, and which line ranges
  were seen (parsed from the read tool's `lines A-B of N` header). Appended after
  the conversation so the cached prefix is untouched; never persisted.
* **Repeat block** (`agent.rs`): the third identical answer still carries a
  warning; the fourth identical **read-only** call with no write in between is
  refused with a `[blocked: ...]` tool result instead of re-billing the same page.
  A mutation resets the check, so re-reading after an edit is always allowed.

### Verified live (real model, real gateway, release binary)

```
request 1  role=user    ledger=False   <- the real prompt
request 2  role=system  ledger=True
request 3  role=system  ledger=True   nudge=True   (3 silent turns)
```

tool results: three identical `read_file` answers, then
`[blocked: this exact call has already returned the same result 3 times…]`, then
the model wrote prose ("Three of the four identical read_file calls are done;
`sample.txt` is 59 lines…") and finished with `DONE`.

### The trap this hit (kept as a regression)

The ledger was first appended as a `user` turn. That makes it "the newest user
message", so `scripts/mock_openai.py` (and any consumer that keys off the last
user message - a real model included) stopped seeing this turn's tool results and
the e2e run looped forever. The note is a `system` turn now, and
`the_request_says_what_already_been_read` asserts the role. `e2e_smoke.py` is the
guard: it fails loudly if the tail ever becomes a user message again.

### Round-2 verification

| check | result |
| --- | --- |
| `cargo test -p kc-runtime --lib` | 113 passed |
| `cargo test --workspace` / clippy `-D warnings` | 30 suites ok / clean |
| `cargo run -p kc-docs -- --check` | ok |
| `scripts/e2e_smoke.py` (release + debug) | 38 passed |
| live gateway run | ledger + nudge delivered, 4th read blocked, prose produced |

New tests: `a_written_finding_survives_compaction`,
`findings_are_released_oldest_first_once_they_exceed_the_note_budget`,
`the_read_ledger_names_files_ranges_and_counts`,
`the_request_says_what_already_been_read`,
`an_identical_read_is_blocked_instead_of_re_executed`,
`a_silent_model_is_told_to_write_its_findings_down`.

### Still open (deliberately not done)

A read-only review has nowhere durable to put findings except the message text,
which is bounded by the window. The next step, if the loop persists, is an
explicit **notes channel** (a notebook the model appends to, kept outside the
message window and re-injected like the ledger) so a 1.7 MB review can accumulate
findings the way a human's notes do. That is a new tool plus a digest policy, not
a tweak.

## Round 3 - one budget, and it belongs to the model

Toil: *"Why do we have another context limit? Everything should be done via the
provider/model configuration."* Correct - `limits.context_max_chars = 240000` was
being applied to every model on the install, including a 1M-token one.

### What was wrong

`ModelEntry` (the `[[providers.models]]` table, prefilled from models.dev) has
`context_window` and `max_output_tokens`, but `ModelConfig` - what a run actually
resolves - carried neither. The runtime therefore had no idea how big the model
was and filled the hole with a global constant. On this install that constant was
**4.3x smaller** than the window (240k chars ≈ 60k tokens against a 1,048,576
token model), so every long run compacted - and re-read - context it had room to
keep.

### What changed

* `ModelConfig` gained `context_window` / `max_output_tokens`, filled from the
  matching `ModelEntry` in `Config::model_for_profile`; `[model]` can set them
  directly too.
* `Limits::context_chars_for(&model)` is now the **only** place a budget comes
  from: `(context_window - output room) x 4 chars/token x 0.95`, with a 32k floor.
  An unknown window falls back to 240k; that is now the *fallback*, not the rule.
* `limits.context_max_chars` defaults to **0 = derive from the model**. A
  non-zero value is an install-wide ceiling: it can lower the derived budget but
  never raise it past what the model accepts. A deliberately tiny explicit value
  is still honoured exactly (the old tests depend on it).
* The loop carries the resolved number (`LoopSpec::context_max_chars`) instead of
  reading a limit in four different places, so the request, the run window, the
  per-turn cap, the ledger reserve and the startup repair cannot disagree.
* Settings API accepts `0`; the settings sheet labels the field
  "Context budget (chars, 0 = from the model)" and only that field allows 0.

### Effect on this install

| model (from the live profile) | window | output | derived budget |
| --- | --- | --- | --- |
| `umans/umans-deepseek-v4.1-flash` (pinned) | 1,048,576 | 393,215 | **2,490,371** chars |
| `surplus/deepseek-v4.1-flash-1017` | 1,000,000 | 384,000 | 2,340,800 chars |
| `surplus/glm-5.3` | 1,000,000 | 131,072 | 3,301,926 chars |
| `gpt-4o-mini` (no window declared) | - | - | 240,000 (fallback) |

10.4x more room for the pinned model, and the review transcripts that used to
compact 38 times fit in one window (0.69 MB against 2.49 MB).

### Verification

`cargo test -p kc-core` 51 (4 new: window-derived budget, unknown-window
fallback, explicit ceiling clamped/honoured, profile window reaches the resolved
config), `cargo test -p kc-runtime --lib` 114 (`the_context_budget_comes_from_the_models_window`
seeds a 320k-char history that the old default would have digested and asserts it
survives under a 200k-token model, plus the same history under an explicit 240k
ceiling still compacts - so the assertion measures the budget, not a no-op),
30 workspace suites, clippy `-D warnings`, `kc-docs --check`, web `tsc` + 423
tests, `e2e_smoke.py` 38/38 on the release binary.

## Round 4 - the loop was ours, and it was two request fields

Toil: *"I don't have this issue with this provider and model in PI, only in
karucode"*, then *"it happens whenever a main agent goes to write a file and
talks about it via reasoning"*, then *"PI doesn't ask us to set temperature but
our harness forces us to set"*.

The second and third remarks were both right, and they compound.

### Method

The looping session (`s_aacca69c…`, project `/home/karutoil/projects`, cancelled
after 390s with a three-minute turn that persisted nothing) was rebuilt into the
exact request karucode would have sent: 140 messages, 198,294 chars, the real
system prompt and the 10 real tool schemas. That body was then replayed against
the live gateway with one field changed at a time. Deterministic: repeated runs
produced identical token counts.

### What the replay showed

At the point where the model had to write a file:

| request | result |
| --- | --- |
| history alone (pi-like) | `write_file(8308B)` - 4.0k tokens |
| + our runtime note at the end (`system`) | 8.0k tokens, reasoning only, **no tool call** |
| + the same note folded into the last tool result | 8.0k tokens, reasoning only, **no tool call** |
| + the same note at the FRONT of the request | `write_file(10817B)` - commits |
| history alone, `temperature` **omitted** | `write_file(14433B)` - commits |
| history + note, `temperature: 0.2` | **8.0k tokens, no call, `finish=length`** |
| history + note, `temperature` omitted | `write_file(15812B)` - commits |

So there were two, and karucode had both: a **runtime note appended after the
conversation** (the read ledger / silence nudge I added in round 2 - it is *our*
injection that pi has no equivalent of), and a **temperature the caller never
asked for** (0.2 by default, impossible to omit). Either alone survived; together
they turned the write into a 400k-token stall.

### Fixed

* Runtime notes are gone from the request entirely. `ReadLedger`, the silence
  nudge and the note appended to repeated tool results were removed; the request
  is now exactly the conversation, with the tool-call repeat **block** kept
  (a `[blocked: ...]` tool result, which is a turn, not an appended note).
  Pinned by `nothing_is_ever_sent_after_the_conversation`.
* `temperature` is **not sent unless asked for**. `ModelConfig` gained
  `send_temperature` (default `false`); the body omits the field unless it is
  true. `MODEL_SEND_TEMPERATURE` persists it; `config.example.toml` documents it.
  Pinned by `temperature_is_not_sent_unless_it_is_asked_for`.

### Verified on the wire (release binary, logging proxy, live gateway)

```
keys sent: messages, model, parallel_tool_calls, stream, stream_options, tools
temperature: <NOT SENT>
messages: 1 system, last message role = user      (no synthetic tail)
```

Gates: 30 workspace suites, clippy `-D warnings`, `kc-docs --check`, web tsc +
446 tests, `e2e_smoke.py` 38/38 on the release binary. While verifying, a peer
session added seven `Store` methods (retention/vacuum/integrity) and left the
runtime's `FakeStore` without them; the test double now implements them, which is
what made `cargo test --workspace` runnable again.

## The remaining lever is configuration - now resolved by round 3

The first version of this report told the operator to raise
`limits.context_max_chars` by hand, because the harness had no idea how big the
model was. Round 3 removed that job: the budget is derived from the model's own
`context_window`, `context_max_chars` defaults to `0` (auto), and a non-zero
value is only a ceiling. See "Round 3" above for the numbers.

What is still configuration, and legitimately so:

* a model added by hand with **no** `context_window` falls back to 240k chars -
  fill the field in (models picked from the catalog already have it);
* `limits.max_tool_output_bytes` still truncates one tool result at 24 KB. That is
  a per-call cap, not a window: raise it if you routinely read single 50 KB files
  and want them whole.

## Reproducing

```bash
# unit + integration, including the cache-stability invariant
cargo test -p kc-runtime --lib context
cargo test -p kc-runtime --lib      # usage-after-finish, turn cap, repeat note
cargo test --workspace
cargo run -p kc-docs -- --check

# live: point a throwaway server at a gateway and watch one run
RUST_LOG=debug target/debug/karucode --config /tmp/probe/config.toml --no-open
grep 'prompt cache' /tmp/probe/server.log
```

The replay used for the tables above is not committed: it is a Python port of
`context.rs` run over a read-only copy of the database. The invariants it
protects are pinned by `the_window_front_is_stable_between_compactions`,
`compaction_never_drops_the_turn_in_flight`, `the_digest_names_the_files_that_were_read`,
`a_rolled_up_digest_does_not_nest`, `a_turn_cannot_exceed_its_byte_share`,
`a_repeated_identical_call_is_flagged_after_the_third_answer`,
`usage_is_read_when_it_arrives_after_the_finish_chunk` and
`cached_prompt_tokens_are_read_from_every_reported_shape`.
