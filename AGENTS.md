# AGENTS.md — instructions for anyone (or anything) editing karucode

This file is the contract for changes in this repository. Read it before you edit
anything. `docs/CONTRIBUTING-AGENTS.md` adds the build/ownership rules; the rules
below are the ones that must never be skipped.

karucode answers questions about **itself** — how to configure a provider, write a
plugin, wire an MCP server, change approvals, call the HTTP API. Those answers come
from `@kc-doc` comment blocks compiled into the binary, not from a wiki. The agent
reads them through the `karucode_docs` tool. If the block is wrong, the agent is
confidently wrong. So:

## 1. Documentation is part of the change, not a follow-up

**Any change that alters something a user can ask about MUST add, update or delete
the matching `@kc-doc` block in the same change.** No exceptions, no "docs later".

A change triggers this rule when it touches any of:

| You changed… | Where it is documented |
| --- | --- |
| a config key, TOML section or default | the module that reads it (`configuration`, `provider-profiles`, …) |
| an environment variable | the block whose `@kc-applies-to` lists it |
| a CLI flag or startup behaviour | `run-server` |
| an HTTP route, request field or error code | `http-api` |
| a built-in tool, its arguments, class or limit | `builtin-tools` |
| the plugin protocol, manifest, SDK or permissions | `plugins` |
| MCP transport, config field or unsupported feature | `mcp` |
| an approval mode, allowlist or decision | `approvals` |
| project/session/workspace semantics | `projects` |
| a skill, the folders it is found in, or the `skill` tool | `skills` |
| a security control or trust boundary | `security` |
| subagent limits or behaviour | `subagents` |
| a UI panel, shortcut or build step | `web-ui` |
| the documentation system itself | `documentation` |

If a feature is **removed**, delete its block. If the capability is one of the
required topics, also remove its slug from `REQUIRED_SLUGS` in
`crates/kc-docs/src/lib.rs` — that diff is the record of a deliberate decision
rather than a silent loss.

If a change introduces a **new** user-facing capability (a new section, transport,
mode or tool family), add a new block and a slug to `REQUIRED_SLUGS`.

## 2. The block format

Plain `//` comments in the file that implements the behaviour. Metadata first, then
one blank `//` line, then markdown. `///` and `//!` are accepted too (use `///` only
when it is attached to an item, otherwise rustc warns about an unused doc comment).

```rust
// @kc-doc: mcp
// @kc-title: Connect MCP servers
// @kc-summary: One sentence; this is what search results show.
// @kc-tags: mcp, tools, servers, setup
// @kc-applies-to: config:[[mcp_servers]], route:GET /api/mcp, tool:mcp__<server>__<tool>
//
// # A heading the reader will scan for
//
// Concrete steps, complete examples, defaults, and the gotchas the code really has.
```

Rules that the build enforces:

* `@kc-doc:` — stable slug: lowercase letters, digits, `-`, `.`, `_`. Unique in the
  repository. Changing a slug breaks links and trained habits; add a new block
  instead.
* `@kc-title:` and `@kc-summary:` are required; `@kc-tags:` and `@kc-applies-to:`
  are optional but strongly encouraged. Unknown `@kc-…` keys fail the build.
* The body is required and must explain **how to do the task**. A title and a
  summary with no body fails the review even though it compiles.
* `@kc-applies-to` is the search index for identifiers: every env var, config key,
  route, tool name and file the doc is about belongs there. That is how a user who
  pastes `KARUCODE_AUTH_TOKEN` lands on the right page.

Write for the agent first and the human second:

* one task per block; if it needs two unrelated procedures, write two blocks;
* copy-pasteable examples, not fragments;
* say what is refused, what the default is, what costs money or time;
* link the deep reference (`docs/PLUGINS.md`, `docs/API.md`, `docs/MCP.md`)
  instead of duplicating it;
* **never document an intention the code does not implement.** The doc compiles
  from the same revision as the code, so this system can catch a stale flag but
  cannot catch a confident lie.

## 3. Commands

```bash
cargo run -p kc-docs -- --write    # regenerate docs/internal (commit the result)
cargo run -p kc-docs -- --check    # what CI runs; fails on drift or bad blocks
cargo run -p kc-docs -- list       # every topic, title, source file:line
cargo run -p kc-docs -- search "configure a provider"
cargo run -p kc-docs -- read mcp
cargo test -p kc-docs              # parser, search and quality-gate tests
python3 scripts/e2e_docs.py        # real server + scripted provider (needs a built kc-server)
```

`cargo build` compiles the blocks into the binary, so a malformed block fails the
build with a `path:line: message` diagnostic even if you forget the commands above.

## 4. Definition of done

A change is not done until all of these hold:

1. `cargo test -p kc-docs` passes (required topics present, bodies substantive,
   search finds them).
2. `cargo run -p kc-docs -- --check` passes, and `docs/internal/` is committed
   with the code change.
3. `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`
   pass (see `docs/CONTRIBUTING-AGENTS.md` for per-crate shortcuts).
4. The block answers the question a user would actually ask. Re-read it as
   someone who has never seen this repository.

## 5. Windows is a supported target — do not regress it

karucode is being ported to Windows (`x86_64-pc-windows-msvc`). The work order,
the evidence and the milestones are in [`docs/WINDOWS-SUPPORT.md`](docs/WINDOWS-SUPPORT.md).
Read it before touching process spawning, paths, the terminal, or packaging.

CI enforces the cheap half already: the **`windows cross-check`** job in
`.github/workflows/ci.yml` cross-checks every crate that is already
Windows-clean (`kc-core`, `kc-store`, `kc-provider`, `kc-runtime`,
`kc-plugin-sdk`, `kc-plugin-host`, `kc-mcp`, `kc-docs`, both plugins) for
`x86_64-pc-windows-gnu` on every push and PR. **If your change adds un-gated
POSIX code to one of those crates, that job goes red and your change is not
done.** `kc-tools/src/command.rs` and `kc-server/src/terminals.rs` are the two
files still being ported; they are checked in a second, non-blocking step that
reports the remaining error count. Do not add new POSIX code to them either —
shrink that number, never grow it.

The rules:

* **Never add un-gated `libc::`, `std::os::unix`, `pre_exec`, `setsid`, `kill`,
  `openpty`, `ioctl`, `PermissionsExt`/`from_mode`, or a hardcoded `/bin/sh`,
  `/tmp` or `$HOME` to library or binary code.** Gate it `#[cfg(unix)]` and write
  the `#[cfg(windows)]` arm in the same change.
* **A `#[cfg(not(unix))]` arm that returns `Ok(())` or `false` is a promise you
  are breaking silently.** This codebase already has several (process-group
  teardown, attachment `0600`, the store's writer lock). Either implement the
  Windows behaviour or make the feature report itself unavailable — never leave
  a no-op that a doc block still describes as working.
* **Process trees:** use the shared process-group seam, not fresh `setsid`/`kill`
  code. Windows has no process groups and no `SIGTERM`; killing a tree needs a
  Job Object. There are four spawn sites (`kc-tools/src/command.rs`,
  `kc-plugin-host/src/rpc.rs`, `kc-mcp/src/stdio.rs`,
  `kc-server/src/terminals.rs`) and they must not drift apart.
* **Paths are a security boundary.** Windows paths are case-insensitive, carry
  drive prefixes and `\\?\` verbatim forms, and have junctions as well as
  symlinks. Any change to `kc-core/src/paths.rs` needs tests for those, not just
  the Unix ones.
* **Spawning a program by name:** `CreateProcess` does not apply `PATHEXT`, so
  `npm`, `npx` and `uvx` are `.cmd` files on Windows and will not launch. Resolve
  through `PATH`×`PATHEXT`.
* **Tests:** a test you gate `#[cfg(unix)]` needs a Windows counterpart asserting
  the *same guarantee*, or coverage silently drops.
* **Docs:** a platform-specific claim in an `@kc-doc` block (`/bin/sh -c`,
  SIGHUP, `0700`, systemd, `~/.config`) must gain its Windows wording in the same
  change — see §1 of this file. Do not document Windows behaviour the code does
  not have yet.

To check locally before pushing (this is the hard gate CI runs):

```bash
rustup target add x86_64-pc-windows-gnu
# Debian/Ubuntu: sudo apt-get install -y mingw-w64
KARUCODE_SKIP_WEB_BUILD=1 \
CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++ \
AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar \
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
cargo check --target x86_64-pc-windows-gnu --lib --bins \
  -p kc-core -p kc-store -p kc-provider -p kc-runtime \
  -p kc-plugin-sdk -p kc-plugin-host -p kc-mcp -p kc-docs \
  -p example-hello -p session-status
```

## 6. Do not

* Add a feature and leave its docs to the next agent.
* Edit `docs/internal/*.md` by hand — it is generated and `--check` will fail.
* Put a doc block in a test fixture or a string literal; the scanner only accepts
  real comments (it lexes strings and block comments).
* Document the workspace's own files in these blocks. They describe karucode, not
  the user's project.
* Rewrite another agent's crate; see `docs/CONTRIBUTING-AGENTS.md`.