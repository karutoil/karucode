# Internal documentation: how it works

karucode ships its own documentation **inside the binary**. It is written as
`@kc-doc` comment blocks next to the code that implements each behaviour, compiled
at build time by `crates/kc-docs/build.rs`, and read by the agent through the
read-only `karucode_docs` tool.

Why this exists: the most common wrong answer a coding agent gives about karucode
is a confident, stale one — a flag that was renamed, a route that moved, a plugin
permission that no longer exists. Keeping the documentation in the same file, and
compiling it from the same revision, means the only way to ship a stale answer is
to ship stale code.

* Contract for contributors and agents: [`AGENTS.md`](../AGENTS.md)
* Implementation: [`crates/kc-docs`](../crates/kc-docs)
* Agent tool: [`crates/kc-tools/src/docs_tool.rs`](../crates/kc-tools/src/docs_tool.rs)
* Generated copy: [`docs/internal/`](internal/)

---

## 1. Writing a block

The block is a run of plain `//` comments. Metadata lines come first, then one
blank comment line, then Markdown.

```rust
// @kc-doc: approvals
// @kc-title: Control what the agent may do without asking
// @kc-summary: The four approval modes, the allowlists behind them, and how a pending approval is answered.
// @kc-tags: approvals, permissions, safety, policy
// @kc-applies-to: config:[approvals], env:KARUCODE_APPROVAL_MODE, route:POST /api/approvals/:id
//
// Body starts here. Markdown, fenced code, tables — anything.
```

| Metadata | Required | Purpose |
| --- | --- | --- |
| `@kc-doc: <slug>` | yes | Stable identifier and file name (`docs/internal/<slug>.md`) |
| `@kc-title:` | yes | The heading, shown in the topic list |
| `@kc-summary:` | yes | One sentence, shown in search results (≤240 chars) |
| `@kc-tags:` | no | Comma-separated keywords |
| `@kc-applies-to:` | no | Comma-separated identifiers: `env:`, `config:`, `route:`, `tool:`, `file:`, `ui:`, `header:`, `cmd:`, `doc:` |

Optional but cheap, and the highest-leverage part of the whole system:
`@kc-applies-to` is what makes `/api/mcp`, `KARUCODE_AUTH_TOKEN` or
`[[mcp_servers]]` findable verbatim.

Metadata is only recognised **before** the body begins. A blank comment line ends
the metadata section, so a body can quote `@kc-…` lines — including the example
above — without confusing the parser.

### Rules the build enforces

* slug: lowercase letters, digits, `-`, `.`, `_`; 1-80 chars; no leading `-`/`.`
  and no trailing `-`/`.`; **unique across the repository**.
* `title` ≤140 chars, `summary` ≤240 chars, body non-empty.
* Unknown `@kc-…` key, malformed line, duplicate key in one block, duplicate slug:
  fail the build with `path:line: message`.
* Bodies of required topics must be at least 400 characters
  (`MIN_REQUIRED_BODY_CHARS`): a stub does not answer a question.

### What counts as a comment

Only comments a compiler would see. The scanner lexes the file: a `// @kc-doc:`
inside a Rust raw string, a normal string, a char literal, a TypeScript template
literal or a `/* … */` block comment is ignored. That is what lets the parser's own
test fixtures look like real doc blocks without being published.

Accepted comment prefixes: `//`, `///`, `//!`. Files scanned: `*.rs`, `*.ts`,
`*.tsx`, `*.js`, `*.jsx` under `crates/`, `plugins/` and `web/src/`. `examples/`,
`target/`, `node_modules/` and `dist/` are skipped.

## 2. What gets generated

`build.rs` writes a Rust table into `$OUT_DIR/generated_docs.rs`, which
`kc-docs/src/lib.rs` includes. The binary therefore always carries the docs of the
revision it was built from. `cargo:rerun-if-changed` watches the three source
roots, so a new block takes effect without touching the build script.

The human-readable mirror is produced by the CLI:

```
docs/internal/
├── README.md          # index table with links and summaries
├── index.json         # machine-readable: slug, title, summary, tags, applies_to, source
├── approvals.md       # one file per slug, with a "generated, do not edit" header
└── …
```

Commit these files with the source change. `--check` fails when the code and the
mirror disagree, and deletes-into-an-error when a generated file no longer
corresponds to any block. Hand-written files in `docs/internal/` without the
generated header are left alone.

## 3. Commands

```bash
cargo run -p kc-docs -- --write     # regenerate docs/internal
cargo run -p kc-docs -- --check     # CI gate: drift, missing files, bad blocks, quality
cargo run -p kc-docs -- list        # slug, title, source path:line
cargo run -p kc-docs -- read <slug> # one doc as markdown
cargo run -p kc-docs -- search "…"  # ranked search
cargo test -p kc-docs               # parser + registry + search + quality tests
python3 scripts/e2e_docs.py         # real server + scripted provider: the tool, end to end
```

Flags: `--root <dir>` (default: the checkout the binary was compiled from) and
`--out <dir>` (default: `<root>/docs/internal`). No arguments means `check`.

## 4. How the agent uses it

* The system prompt lists every topic slug (`kc_docs::topic_index`) and tells the
  model to search before answering a question about karucode. The pointer is only
  emitted when the tool is actually registered.
* `karucode_docs` is a single read-only tool with three actions:
  * `{"action":"list"}` — every slug, title and summary;
  * `{"action":"search","query":"configure a provider","limit":5}` — ranked hits
    with a snippet and source location;
  * `{"action":"read","doc":"mcp"}` — the full markdown plus its source.
* It is class `ReadOnly`, so it never asks for approval, works in any project, and
  cannot touch the workspace.

Search scores title, slug, tags, `applies_to`, summary and body with different
weights, adds a bonus for an exact phrase, and requires a document to match at
least two thirds of the meaningful query terms (after stop-word removal). Ordering
is deterministic: matched terms, then score, then slug.

## 5. Adding a new topic

1. Write the block in the file that implements the behaviour.
2. Add the slug to `REQUIRED_SLUGS` in `crates/kc-docs/src/lib.rs` if the topic
   must never disappear.
3. Add a test case to `search_finds_the_obvious_questions` in the same file: the
   question a user would ask, and the slug that must rank for it.
4. `cargo run -p kc-docs -- --write`, then `--check`.
5. Commit the source, the required-slug change, the test and `docs/internal/`.

Removing a capability is the mirror image: delete the block, delete the slug, add
a reason in the commit message.

## 6. Troubleshooting

| Symptom | Cause |
| --- | --- |
| `found no @kc-doc blocks under …` | the scanner cannot see the tree (wrong `--root`, or the blocks were deleted) |
| `duplicate @kc-doc slug` | two blocks share a slug; merge them or rename one |
| `unknown metadata key` | typo such as `@kc-titel`; the allowed keys are listed in the error |
| `empty documentation body` | metadata without prose |
| `required topic … has a N-character body` | a stub; write the procedure |
| `--check` reports a file as missing/out of date | run `--write` and commit |
| a block in a test fixture is published | it is inside a `/* */` comment or no longer lexed as a string — move it or use `//` lines inside the string |

## 7. FAQ

**Why not rustdoc?** rustdoc documents the Rust API, not "how do I add an MCP
server". It also cannot document the HTTP contract, the config file or the UI, and
it is not available to the running agent.

**Why not a wiki or a `docs/` page?** Because it drifts silently. The `docs/*.md`
files that exist are the *deep reference*; the blocks are the short, always-current
answer the agent quotes.

**Why one tool instead of three?** Fewer tools means less prompt surface and one
name for the model to learn; the actions are validated and each has an actionable
error.

**Can a human read the docs without the binary?** Yes — `docs/internal/`, or
`cargo run -p kc-docs -- read <slug>`.
