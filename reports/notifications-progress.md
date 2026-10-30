# Notifications — implementation state

Plan: `reports/notifications.md`. This file tracks what is actually done, so a later
session can pick up without re-reading the diff.

## Done: backend (t2, t3, t4, t5, t6, t8, t9, t10, t11)

| Piece | Where |
| --- | --- |
| `Notify` / `NotifyKind` + caps | `crates/kc-core/src/notify.rs` |
| `Event::Notification` | `crates/kc-core/src/events.rs` |
| `NotificationsConfig` + `[notifications]` + env | `crates/kc-core/src/config.rs` |
| `Settings::notify_config()` (defaulted) | `crates/kc-core/src/settings.rs` |
| `NotificationObserver` + `Hooks::notifier()` | `crates/kc-core/src/hooks.rs` |
| settings keys / patch / public config / rows / validation | `crates/kc-server/src/settings.rs` |
| runtime emitter | `crates/kc-runtime/src/notify.rs`, wired in `agent.rs`, `approvals.rs`, `subagent.rs` |
| `cancelled_by_user` flag | `crates/kc-runtime/src/lib.rs` (`Runtime::cancel` only) |
| plugin `notify` frame + rate limit | `crates/kc-plugin-host/src/rpc.rs` |
| `listen_notifications` + `on_notify` hook | `crates/kc-plugin-host/src/lib.rs` |
| plugin → event bridge | `crates/kc-server/src/plugin_notifications.rs`, wired in `bootstrap.rs` |
| SDK `notify` / `on_notify` / `notifications` | `crates/kc-plugin-sdk/src/lib.rs` |
| fixture tools | `tests/fixtures/fake_plugin.sh` |

Verified: `kc-core` 96, `kc-server --lib` 197, `kc-runtime` 148, `kc-plugin-host` 41+38,
`kc-plugin-sdk` 28+4 doc. A scaffolded plugin builds and emits a `notify` frame
(checked by hand in /tmp).

## Done: everything except the final verification (t27–t31)

Backend (t2–t11), web (t12–t22) and docs (t23–t26) are complete. See below.

| Piece | Where |
| --- | --- |
| `Notify` / `NotifyKind` + caps | `crates/kc-core/src/notify.rs` (also holds the `notifications` doc block) |
| `Event::Notification` | `crates/kc-core/src/events.rs` |
| `NotificationsConfig` + `[notifications]` + env | `crates/kc-core/src/config.rs` |
| `Settings::notify_config()` (defaulted) | `crates/kc-core/src/settings.rs` |
| `NotificationObserver` + `Hooks::notifier()` | `crates/kc-core/src/hooks.rs` |
| settings keys / patch / public config / rows / validation | `crates/kc-server/src/settings.rs` |
| runtime emitter | `crates/kc-runtime/src/notify.rs`, wired in `agent.rs`, `approvals.rs`, `subagent.rs` |
| `cancelled_by_user` flag | `crates/kc-runtime/src/lib.rs` (`Runtime::cancel` only) |
| plugin `notify` frame + rate limit | `crates/kc-plugin-host/src/rpc.rs` |
| `listen_notifications` + `on_notify` hook | `crates/kc-plugin-host/src/lib.rs` |
| plugin → event bridge | `crates/kc-server/src/plugin_notifications.rs`, wired in `bootstrap.rs` |
| SDK `notify` / `on_notify` / `notifications` | `crates/kc-plugin-sdk/src/lib.rs` |
| sounds | `web/src/lib/notifySound.ts` |
| desktop | `web/src/lib/notifyDesktop.ts` |
| reducer (four surfaces) | `web/src/state/notifier.ts`, wiring in `notifyWiring.ts` |
| UI | `NotificationsSettings.tsx`, `ToastRegion.tsx` (Open action), `SessionsRail.tsx` (unread), `app.tsx` (title + gesture) |
| fixture tools | `tests/fixtures/fake_plugin.sh` |
| docs | new `notifications` block + `REQUIRED_SLUGS`; updated `configuration`, `http-api`, `plugins`, `web-ui`; `docs/PLUGINS.md` §4.7/§7; `docs/API.md`; `config.example.toml`; `.env.example`; CHANGELOG; PRODUCTION-READINESS |

### Verified so far

- `cargo test --workspace`: **804 passed**, 0 failed by my code. The 4 failures
  (`first_message_titles_an_untitled_session`, `second_message_keeps_the_derived_title`,
  `workspace_explorer_confines_paths`, `workspace_file_can_be_saved_from_the_editor`)
  reproduce on the clean tree — they are pre-existing, not mine.
- `cargo clippy --workspace --all-targets -D warnings`: my changes introduce zero
  errors. The 9 that remain are pre-existing in `crates/kc-server/src/terminals.rs`
  and `routes/terminals.rs` (both committed and untouched here), also present on the
  clean tree.
- `cargo fmt --check`: every file I touched is formatted; 11 pre-existing files drift.
- `cargo run -p kc-docs -- --check`: 19 topics, 21 files up to date.
- Web: `npx vitest run` 64 files / **875 tests pass**, `tsc --noEmit` clean,
  `npm run build` succeeds.
- A freshly scaffolded plugin (`karucode plugin init`) builds against the new SDK and
  emits a well-formed `notify` frame when driven by hand.

### Regression found and fixed during verification

Adding the session-title lookup to `run_top_level` put a store round-trip **before**
`RunStarted`, which delayed the event the API derives "is it running" from. It made
`all_modes_validate_the_prompt` fail roughly 1 run in 6. Fixed by reading the title
only where it is needed (after the loop, for the notification). 8/8 clean after.

## Remaining

Nothing. Every TODO item is closed and every gate was run in the final pass.

## Verification record

| Gate | Result |
| --- | --- |
| `cargo test --workspace` | **804 passed**. The 4 failures (`first_message_titles_an_untitled_session`, `second_message_keeps_the_derived_title`, `workspace_explorer_confines_paths`, `workspace_file_can_be_saved_from_the_editor`) reproduce on a clean `git stash` tree — pre-existing, not from this work. |
| `cargo clippy --workspace --all-targets -D warnings` | Clean for every file touched. The 9 remaining errors are pre-existing in `crates/kc-server/src/terminals.rs` and `routes/terminals.rs` (also on the clean tree). |
| `cargo fmt --check` | Clean for every file touched; 11 pre-existing files drift. |
| `cargo run -p kc-docs -- --check` | ok — 19 topics, 21 files up to date. |
| web `npx vitest run` | **66 files / 917 tests passed**. |
| web `tsc --noEmit`, `npm run build` | Clean; build succeeds. |
| `python3 scripts/e2e_smoke.py` | **46 passed, 0 failed, 0 skipped** — includes six notification checks. |
| `python3 scripts/verify_notifications.py` | **13/13** in a real Chrome against a real server. |

### What the real browser proved (and a unit test cannot)

- the finished cue renders **audible samples** under the page's actual CSP:
  `peak=0.179 rms=0.0162` from an `OfflineAudioContext` — not silence, and no
  `media-src` violation because nothing is a file;
- a live `AudioContext` runs (`state=running`, clock advancing) **while
  `Notification.permission` is still `default`** — the requirement that sound does not
  depend on notification permission;
- nothing asked for a permission on load;
- the settings section renders 14 switches and exactly 4 cue preview buttons, and
  flipping one **reaches the server** through `PATCH /api/config`;
- a real run produced an unread badge (`1`) on the session row and the title
  `(1) Notify me from another tab — karucode`;
- the plugin path: `plugin__notifier-fixture__ping` behind an approval emitted a
  `notify` frame that arrived as `notification` with `source=notifier-fixture`, and
  `plugin = false` refused it.

## Bugs this work found in itself

1. **A delayed `RunStarted`.** Reading the session title before the loop put a store
   round-trip on the run's critical path and made `all_modes_validate_the_prompt` fail
   ~1 run in 6. Fixed by reading it only where it is used.
2. **`dispose()` left the coalesce window open**, so the first cue after a rebuild was
   swallowed. Found by a test, fixed in the engine.
3. **The e2e `run_prompt` helper returns at `run_finished`**, so a notification that
   (correctly) follows it was not collected — a test artefact, fixed by waiting for
   the frame explicitly.



## Facts to keep in mind

- The page CSP has no `media-src`, so it falls back to `default-src 'self'`;
  blob:/data: audio is refused with `NotSupportedError`. Synthesize cues.
- `Notification.permission` and sound are independent — prove it in the tests.
- jsdom has neither `Notification` nor `AudioContext`: inject fakes.
- Server `[notifications] enabled = false` means **no event at all**, not "UI hides
  it". The client does not need a master-switch check for correctness, only for
  the settings UI.
- `config.notifications` in `GET /api/config` carries every switch plus
  `max_min_run_secs` and `kinds`.
