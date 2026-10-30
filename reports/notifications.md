# Notifications: what fires, how it is turned off, and why the sounds are generated

A plan, not a description of shipped code. Everything in **§2 (measured)** was
verified on this host on 2026-10-29 against the running binary; everything about
audio came from a real page served with the real CSP header, not from memory.
Anything marked *decision* is a choice this document makes on Toil's behalf and
can be overruled before implementation starts.

---

## 1. The question

Three asks, in order of how much design they need:

1. **A signal when a turn is done.** Today the UI only knows because an SSE frame
   arrived, and only for the session that happens to be open.
2. **A signal when something is required.** An approval request already exists as
   an event; the problem is that nobody finds out when they are not looking.
3. **A user-visible way to turn all of it off**, and **a sound per notification
   kind** that plays whether or not browser notifications are allowed.

## 2. What is true today (read, not remembered)

| Fact | Where |
| --- | --- |
| No notification code exists anywhere in `web/src`: no `Notification`, no `AudioContext`, no audio assets | repo-wide search |
| The server has no notification event. The nearest signals are `run_finished{status,error}`, `approval_requested`, `agent_finished`, `notice` | `crates/kc-core/src/events.rs` |
| One SSE fan-out already carries every event to every client, with a monotonic `seq`, a replay ring and a lag marker | `crates/kc-runtime/src/events.rs`, `crates/kc-server/src/routes/events.rs` |
| The client already derives "run started / run finished / waiting for approval / tool finished" from `ChatState` — for screen readers only, and only for the open session | `web/src/components/LiveAnnouncer.tsx` |
| An in-app toast channel exists (`store.pushToast`, `ToastRegion`) for transient feedback — saves, copies, theme actions — but nothing long-lived or attention-seeking | `web/src/state/store.ts`, `web/src/components/ToastRegion.tsx` |
| A plugin **cannot** reach the browser: no route mounting, no asset serving, no script execution. Panels are data, not code | `docs/PLUGINS.md` §8 |
| Plugin → host notifications are parsed and then **thrown away** at debug level (`method: "notify"` included) | `crates/kc-plugin-host/src/rpc.rs` `handle_line` |
| Plugin hooks are `before_prompt`, `on_run_end`, `on_tool_result`; `on_run_start`/`on_model_*`/`on_session_create` are reserved and never called | `crates/kc-core/src/plugin.rs` `hooks::IMPLEMENTED` |
| `[titles]` is the closest precedent: a config section, env spelling, settings rows, `public_config` and `PATCH /api/config` all in one place | `crates/kc-core/src/config.rs`, `crates/kc-server/src/settings.rs` |
| "no notifications for long runs" is a listed gap | `docs/PRODUCTION-READINESS.md:448` |

So the missing piece is not a UI component. It is **one first-class signal**, because
plugins have no other way to reach a browser and the client has no way to see a
session it is not showing.

## 3. What browsers actually allow (measured)

These four measurements decide the whole design. Each was run in Chrome against
`http://127.0.0.1:8787` — the real server, so the real security headers.

### 3.1 The page CSP forbids audio files

`CONTENT_SECURITY_POLICY` (`crates/kc-server/src/app.rs:132`) is

```
default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:;
connect-src 'self'; frame-ancestors 'none'
```

There is no `media-src`, so it falls back to `default-src 'self'`. Probe:

```js
const url = URL.createObjectURL(new Blob([wavBytes], {type: 'audio/wav'}));
await new Audio(url).play();
```

Result:

```json
{"blobPlay":"rejected: NotSupportedError",
 "violations":[{"directive":"media-src","blocked":"blob"}]}
```

**Consequence:** `.wav`/`.mp3` cue files cannot be shipped as blob or data URLs.
Two honest options:

* **synthesize the cues with Web Audio** (recommended: no assets, no CSP change,
  and each cue is a few lines of code that a test can render and measure);
* or ship same-origin files in `web/public/cues/` **and** add `media-src 'self'`
  to the CSP on purpose — which also means updating the CSP assertion in
  `crates/kc-server/tests/api.rs` in the same change.

### 3.2 Sound does not depend on notification permission

`Notification.permission` was `"default"` (never granted) throughout the probes and
`AudioContext` was unaffected: `state: "running"`, clock advancing, a probe
oscillator rendered. They are unrelated APIs; browser notification permission
governs only the OS notification surface. **This is the requirement "sounds should
work with and without notifications allowed", and the architecture must not couple
them**: one cue engine fed by the event, and one optional desktop surface fed by the
same event.

### 3.3 Autoplay policy is about *user activation*, not permission

`navigator.getAutoplayPolicy` is absent in this Chrome, so a policy check cannot be
used as a gate. What matters, per MDN's autoplay guide, is that
`AudioContext`/`play()` outside a user gesture in a tab that has never been
interacted with is blocked. A workspace is always interacted with (you type a
prompt), so in practice the first gesture just needs to be captured: create the
context lazily, and `resume()` it from a document-level `pointerdown`/`keydown`
listener. If it is still `suspended` when a cue is due, degrade visibly (§6).

### 3.4 A hidden tab keeps its audio clock and its timers

Probed with the tab in the background: `visibilityState: "hidden"`,
`AudioContext.state: "running"`, audio clock advanced 2.005 s over 2.000 s of wall
time, and 40/40 expected 50 ms timer ticks fired. So a run that finishes while the
user is in another window is not a scheduling problem — the event arrives, the cue
plays, and only the *desktop* notification needs the page to still be open.

### 3.5 Two API facts that shape the UI

* `new Notification()` is a **secure-context** API. `http://127.0.0.1` and
  `http://localhost` are treated as secure; a LAN address over plain HTTP is not,
  so the settings screen has to say "not available here" instead of failing
  silently. karucode speaks plain HTTP and expects a TLS terminator in front
  (`docs/SECURITY.md:172`).
* `requestPermission()` must be called from a **user gesture** (Firefox ≥ 72,
  Safari already refuse otherwise), and on most mobile browsers the constructor
  throws `TypeError` — persistent notifications need a service worker. **Decision:**
  desktop notifications are a best-effort desktop surface; mobile gets the in-app
  badge, the title counter and the sound. Registering a service worker is out of
  scope for this change and listed in §8.

### 3.6 What the tests can and cannot assert

`jsdom` provides neither `AudioContext` nor `Notification`. Probe:

```
AudioContext in jsdom: undefined
Notification in jsdom: undefined
```

So vitest can only test the *decisions* (which cue for which event, mute, suspended
context, no unhandled rejection) against an injected fake. Actual sound rendering is
verified in a real browser with `OfflineAudioContext`, which works and is
measurable — the same probe returned `peak 0.298, rms 0.0411` for a 440 ms tone,
which is exactly how "the cue is audible and not silent/clipping" gets asserted.

## 4. Terminology: "turn" in karucode

karucode has **runs**, not turns. One run is a user prompt plus every iteration,
tool call, auto-continue and subagent it spawns; a *turn* is one model iteration
inside it. Notifying per turn would ping on every tool loop — unusable. The design
maps the ask to the states a human actually cares about:

| Moment | Event today | Notification kind | Fires when |
| --- | --- | --- | --- |
| The run settled: `RunStatus::Completed` | `run_finished{status:"completed"}` | `run_finished` | the whole run is done |
| The run settled badly: `failed` / `interrupted` | `run_finished{status}`, `error` | `error` | failure or post-restart interruption |
| The run stopped because it hit a limit | `run_finished{status:"limit_reached"}` | `needs_input` | "wants to continue, may not" |
| The agent is blocked on a human | `approval_requested` | `needs_input` | immediately, and it is the one kind that is urgent |
| The user cancelled it themselves | `run_finished{status:"cancelled"}` | *none* | you already know, you pressed Escape |
| A subagent failed | `agent_finished{status:"failed"}` | `error` | failure only; a successful subagent is noise |
| A plugin asked for attention | plugin `notify` frame | whatever the plugin says | gated by `notifications.plugin` |

`min_run_secs` (default `0`) suppresses notifications for runs shorter than a
threshold, for people who do not want a ping for a two-second question.

## 5. The design: one signal, four surfaces

### 5.1 The payload (`kc-core/src/notify.rs`, new)

```rust
pub enum NotifyKind { RunFinished, NeedsInput, Error, Info }   // "run_finished" | ...

pub struct Notify {
    pub id: String,                 // stable per source moment; the UI dedupes on it
    pub kind: NotifyKind,
    pub title: String,              // "Run finished" — short, UI may override
    pub body: String,               // first line of the answer / the approval title
    pub session_id: String,
    pub session_title: String,      // so a notification is readable on its own
    pub source: String,             // "runtime" or the plugin's name
    pub sound: Option<String>,      // cue override; None = the kind's default
    pub sticky: bool,               // desktop notification survives until dismissed
}
```

`title`/`body` are **plain text**, capped (e.g. 120 / 400 chars) on the way in,
never Markdown, never HTML. A notification is rendered by the operating system, so
this is the one place where "it is just a string" is genuinely true — and the one
place a plugin could try to smuggle something. Caps live in the constructor so no
call site can bypass them.

Why a `kind` and not a free string: the cue table, the per-event settings toggles
and the settings UI all need a closed set. Plugins get the four values, not their
own labels.

### 5.2 The event (`kc-core/src/events.rs`)

```rust
Notification { notification: Notify }
```

serialises as SSE `{"type":"notification", ...}` with the usual envelope
(`seq`, `ts_ms`, `session_id`). Emitted through the existing `RuntimeInner::emit`,
so it inherits monotonic `seq`, the replay ring, the lag marker and the client's
dedupe — a notification cannot arrive out of order or twice after a reconnect.
Additive: an older client ignores an unknown `type` (verified: both reducers
`default:` branches ignore it), so this is not a protocol break.

### 5.3 Where it is emitted

Inside the runtime, at the same places that already publish the underlying event:

* `crates/kc-runtime/src/agent.rs` — after `on_run_end(...)` and next to the
  `Event::RunFinished` emit, so the store write is already done and the status is
  final.
* `crates/kc-runtime/src/approvals.rs` — next to `Event::ApprovalRequested`, for
  the app-level agent only (`agent_id.is_none()`): a subagent's approval is raised
  *through* its parent's card, and two notifications for one card is a bug.
* `crates/kc-runtime/src/subagent.rs` — only for a failed subagent.

**Ordering rule:** the notification is emitted after the state it describes is
persisted, and never inside a lock or before the `run_finished` frame. A
notification is a *hint about* state; if the client has to refetch, the refetch
must already be correct.

**Never emitted inline with a gated action:** the emitter reads config, builds the
payload and calls `emit` — no I/O, no store call, no plugin call. The `on_notify`
hook fan-out (§5.5) is spawned, not awaited, so a slow plugin cannot delay the end
of a run (the same rule `[titles]` follows: "it never blocks the run").

### 5.4 Configuration (`[notifications]`)

Precedence is karucode's existing one: SQLite overrides > env > TOML > defaults.

```toml
[notifications]
enabled = true          # master switch for server-emitted notifications
run_finished = true     # the four per-event toggles
needs_input  = true     # approvals, limit_reached
error        = true     # failed / interrupted runs, failed subagents
info         = true     # plugin info notifications
plugin       = true     # let plugins raise notifications at all
min_run_secs = 0        # skip notifications for runs shorter than this
say_body     = true     # include a line of the answer as the body (false = title only)
```

* `KARUCODE_NOTIFICATIONS` (bool) sets `enabled`, plus
  `KARUCODE_NOTIFICATIONS_EVENTS` (comma list, e.g. `run_finished,needs_input`) and
  `KARUCODE_NOTIFICATIONS_MIN_RUN_SECS` — so a locked deployment can silence it
  from the environment without editing TOML.
* Settings rows `notifications.*` in `crates/kc-server/src/settings.rs`: read in
  `public_config`, written in `apply_row`, patchable through `ConfigPatch`
  (`NotificationsPatch`, `deny_unknown_fields` like every other patch), validated in
  one `validate_notifications` (min_run_secs bound, unknown event name rejected).
* `Settings` trait gains `notify_config()` **with a default implementation**, like
  `resolve_model`, so the five existing test `Settings` impls keep compiling.
* What `enabled = false` means: **the server stops emitting `notification` events
  at all.** Not "the UI hides them". The client never has to trust itself to obey.

Three independent layers of "off", because they answer different questions:

| Layer | Turns off | Lives in |
| --- | --- | --- |
| `[notifications] enabled = false` | everything, for every browser, from the server | config / env / `PATCH /api/config` |
| `notifications.plugin = false` | plugin-raised notifications only | same |
| Per-browser prefs (sound, volume, desktop notifications) | the cue and the OS popup, but keeps in-app toasts and badges | `localStorage` |

### 5.5 The plugin path

Plugins cannot reach a browser, so the server is the only way in — and the only way
out:

**In: `notify` (plugin → host, a JSON-RPC notification).**

```json
{"jsonrpc":"2.0","method":"notify",
 "params":{"kind":"needs_input","title":"Deploy needs a yes",
           "body":"staging is approved, production is not","sound":"ping","sticky":true}}
```

* Handled in `kc-plugin-host/src/rpc.rs::handle_line`, which today logs and drops
  it. Validated exactly like a manifest field: `kind` must be one of the four,
  `title`/`body` go through the same caps, `session_id` may be omitted (the host
  fills in the session the plugin was called for, when there is one), unknown keys
  ignored.
* A malformed `notify` frame is logged and dropped. It must never crash the host,
  poison the slot or fail a run — a plugin is untrusted input on a channel that
  also carries tool results.
* Gated by `notifications.plugin` **at the host**, so a disabled switch cannot be
  bypassed by a plugin that keeps sending.
* The resulting `Event::Notification` has `source = "<plugin name>"`, so the UI can
  say where it came from and can render a generic icon.
* An approval requested by a plugin tool is *not* a plugin notification: that
  already arrives as `approval_requested` and is notified as `needs_input`. No
  double ping.

**Out: `on_notify` (host → plugin hook).** Adding `ON_NOTIFY` to
`kc_core::plugin::hooks` and `IMPLEMENTED`, called with
`{session_id, run_id, kind, title, body, source}` on every emitted notification.
That is what lets a plugin bridge to ntfy/Pushover/a desktop notifier without
karucode growing those integrations. 5 s budget, failures ignored, never awaited by
the run.

**SDK.** `PluginBuilder::notify(...)` (advertises the capability) and an
`sdk::notify(kind, title, body)` helper that writes the frame to stdout. Documented
with a runnable snippet in `docs/PLUGINS.md` §4 (new method) and §7 (hook table),
and exercised through `tests/fixtures/fake_plugin.sh`, which already speaks
`log`, `panels/read` and `hooks/run`.

### 5.6 The four surfaces (web)

One reducer, `web/src/state/notifier.ts`, subscribed to the same stream as
everything else, so "off" means off in one place:

```
notification event ──► notifier.ts ──┬─► in-app toast (always, if enabled)
                                     ├─► session badge + document.title count
                                     ├─► sound cue (browser pref)
                                     └─► desktop Notification (permission + hidden/open-session rule)
```

1. **In-app toast** — a new `attention` tone on `store.pushToast` (today's toasts
   are transient feedback: "Saved …", "Copied …") plus a "Jump to session" action.
   The most reliable channel, so it is the default.
2. **Session badge** — an unread dot on the session row (the rail already has
   `running`/`failed`/`interrupted` tones to hang it on) plus an unread count in
   `document.title`, which is the only thing visible from another tab.
3. **Sound** — §6.
4. **Desktop notification** — only when `Notification.permission === "granted"`
   **and** (`document.hidden` **or** the notification is not about the open,
   focused session). Options: `tag` (replace, never stack), `silent: true` when the
   app already played a cue (never two sounds for one event), `requireInteraction`
   from `sticky`, click → focus the window and open that session. Permission is
   requested **only from a button in settings**, never on load.

Dedupe: the `Notify.id` plus the session, so a reconnect, a replayed frame or two
tabs cannot produce two cues. Focus state is read *at emit time*: the run that
finishes for another project must notify even with the tab in front.

### 5.7 Sounds

`web/src/lib/notifySound.ts`: one `AudioContext` created lazily, `resume()`d from
the first user gesture, cues synthesized (no files, per §3.1) with an attack/decay
envelope so nothing clicks:

| Kind | Cue | Shape |
| --- | --- | --- |
| `run_finished` | two rising notes | 660 → 880 Hz, ~90 ms each, soft sine |
| `needs_input` | double ping | 880 Hz twice, 120 ms apart — the "look at me" cue |
| `error` | low buzz | 220 Hz triangle with slight detune, ~250 ms |
| `info` | single tick | 1200 Hz, 40 ms, quiet |

Rules:

* Volume and mute are browser prefs (`localStorage`), independent of the server
  config and of `Notification.permission`.
* If the context is `suspended` when a cue is due: `resume()`, and if that still
  fails, **no unhandled rejection** and one toast ("Click anywhere to enable
  sounds"). Silent failure that looks like a bug is the failure mode to avoid.
* Cues are non-blocking and dropped, not queued, if one is already playing for the
  same kind (a burst of five approvals is one ping).
* Every cue has a **Preview** button in settings.

### 5.8 Settings

A new section in the settings sheet, in its own component
(`web/src/components/NotificationsSettings.tsx`) rather than another 300 lines in
`SettingsSheet.tsx`, whose own section layout it reuses:

* **Master switch** (server), per-event toggles, plugin switch, minimum run seconds.
* **Sound**: on/off, volume, and a Preview button per cue.
* **Desktop notifications**: a state line ("allowed", "blocked — the browser will
  not ask again, change it in site settings", "not available here: this page is not
  a secure context", "unsupported in this browser") and one button that requests
  permission from a gesture.
* What cannot be changed (locked settings) says so, like every other section.

## 6. Failure modes, and what each does

| Failure | Behaviour |
| --- | --- |
| Notifications disabled | no `notification` event at all; nothing in the UI has to cooperate |
| `Notification` unsupported (mobile, insecure context) | settings says why; toast + badge + sound still work |
| Permission `denied` | never re-prompt; settings explains how to change it at the browser level |
| Permission `default` | nothing is requested until the button is pressed |
| Audio context suspended (no gesture yet) | `resume()`, else one hint toast; no exception reaches the console |
| Audio context construction throws | sound is marked unavailable for the session; notifications continue |
| Plugin sends a malformed `notify` | logged at debug, dropped, host unaffected |
| Plugin floods `notify` | per-plugin rate limit (decision: 1 per second, burst 5), then dropped with one warning |
| Event stream reconnects / replays | `seq` + `Notify.id` dedupe; no double cue |
| Run finishes while the tab is closed | nothing to notify; the rail and the title counter are correct on return |

## 7. Security and noise rules (non-negotiable)

* Notification text is **plain text with caps**, never Markdown/HTML; it is drawn
  by the OS, but the body also lands in a toast, which is the app's origin.
* A plugin cannot set a URL: no `icon`, no `image`, no `actions`, no click target
  beyond "open this session". That is the same rule that keeps panels data-only.
* No notification ever contains model output beyond the first line
  (`say_body = false` turns that off entirely) — a prompt-injected answer must not
  be able to paint an OS notification with arbitrary text.
* Notifications are not an approval channel. Nothing can be *approved* from a
  notification; the card is still the only control.
* Approval notifications are rate-limited to the number of pending cards, and the
  `sticky` flag is on for them (they should not vanish before they are read).

## 8. Decisions for Toil to confirm

1. **Defaults:** `enabled = true`, all four events on, `min_run_secs = 0` — i.e. it
   pings you after every run until you turn it off. The alternative is on but silent
   by default (`sound = false`), which is quieter but easy to never discover.
2. **Cancel is silent** (you pressed Escape; you know). Should a *timeout* cancel
   notify? Recommendation: treat it as `error`.
3. **Service worker for mobile persistent notifications:** out of scope. Adds a
   worker, a scope and a CSP change for a surface karucode's own UI does not target.
4. **Sound cue files vs synthesis:** synthesis, for the CSP reason in §3.1. It also
   makes "the cue is audible" a real assertion instead of a filename.
5. **Plugin rate limit** of 1/s burst 5, or unlimited?
6. Whether the notification should carry the *first line of the answer*
   (`say_body`): useful, but it puts model output on the lock screen.

## 9. Verification (what "done" will mean)

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p kc-docs && cargo run -p kc-docs -- --check
cd web && npm test && npm run typecheck && npm run build
python3 scripts/e2e_smoke.py --binary target/debug/karucode
```

* Unit: `Notify` caps and kind round-trip; config precedence and validation;
  runtime emits exactly one notification per terminal run, **zero** when disabled;
  a failed subagent notifies, a successful one does not; the plugin `notify` frame
  becomes an event, a malformed one does not; `on_notify` is called with the
  payload.
* Web unit: the reducer's four surface decisions (muted, hidden, wrong session,
  dedupe, permission denied) against injected `Notification`/`AudioContext` fakes.
* `scripts/e2e_smoke.py`: the `notification` event arrives for a completed run, is
  absent when `[notifications] enabled = false`, and a plugin-raised one carries
  `source`.
* Real browser (a screenshot is not enough for audio): a run in the background tab
  plays a cue — verified with `OfflineAudioContext` peak/RMS as in §3.6, with
  `Notification.permission` left at `default` to prove the two are independent.
* Docs: a new `notifications` `@kc-doc` block (required slug), plus the
  `web-ui`/`plugins`/`configuration`/`http-api` blocks whose facts changed, and
  `docs/internal/` regenerated with `--write`.

## 10. Files this touches

| Area | Files |
| --- | --- |
| Core | `crates/kc-core/src/notify.rs` (new), `events.rs`, `config.rs`, `settings.rs`, `plugin.rs`, `lib.rs` |
| Runtime | `crates/kc-runtime/src/{agent,approvals,subagent,events}.rs` |
| Server | `crates/kc-server/src/settings.rs` (keys, patch, public config, validation), `state.rs` |
| Plugin host | `crates/kc-plugin-host/src/rpc.rs` (`notify`), `lib.rs` (`on_notify`), `src/tests.rs`, `tests/fixtures/fake_plugin.sh` |
| SDK | `crates/kc-plugin-sdk/src/lib.rs` |
| Web | `src/lib/notifySound.ts` (new), `src/lib/notifyDesktop.ts` (new), `src/state/notifier.ts` (new), `src/state/store.ts` (attention tone + unread signal), `src/state/actions.ts` (feed the reducer), `src/components/{ToastRegion,SessionsRail,SettingsSheet}.tsx`, `src/components/NotificationsSettings.tsx` (new), `src/app.tsx`, `src/styles/settings.css` |
| Docs | the `@kc-doc` blocks above, `docs/internal/*` (generated), `docs/PLUGINS.md`, `docs/API.md`, `config.example.toml`, `.env.example`, `CHANGELOG.md`, `docs/PRODUCTION-READINESS.md:448` |