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

## 5. Do not

* Add a feature and leave its docs to the next agent.
* Edit `docs/internal/*.md` by hand — it is generated and `--check` will fail.
* Put a doc block in a test fixture or a string literal; the scanner only accepts
  real comments (it lexes strings and block comments).
* Document the workspace's own files in these blocks. They describe karucode, not
  the user's project.
* Rewrite another agent's crate; see `docs/CONTRIBUTING-AGENTS.md`.