# Contributing to karucode

karucode is a Rust workspace plus a Preact frontend that is embedded into the
binary at build time. This is the short human guide; the deeper contract for a
change — including the rules several cooperating agents follow in one checkout —
is [AGENTS.md](AGENTS.md) and [docs/CONTRIBUTING-AGENTS.md](docs/CONTRIBUTING-AGENTS.md).

## Prerequisites

* Linux (the primary target — the shell tool's process-group code is
  Linux-specific; macOS runs the unit-test matrix but is otherwise untested,
  Windows is unsupported).
* Rust 1.89+ (the workspace `rust-version`; MSRV, checked by CI).
* Node.js 22.22+ — **only to build the frontend**; `cargo build -p kc-server`
  runs `npm ci` + `npm run build` itself and embeds the result. Nothing Node
  remains at runtime. `KARUCODE_SKIP_WEB_BUILD=1` skips the frontend entirely for
  Rust-only iteration.

## Build

```bash
cargo check --workspace          # backend
cargo test --workspace           # unit + integration tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

cd web
npm install
npm run dev                      # Vite on :5173, proxies /api -> :8787
npm test                         # vitest component/reducer tests
npm run typecheck
npm run build                    # emits web/dist (embedded on the next cargo build)
```

## The documentation contract

Documentation about karucode itself is compiled from `@kc-doc` comment blocks in
the source, so it cannot drift from the build. **Any change that alters
something a user can ask about — a config key, an environment variable, a route,
a tool, a plugin or MCP behaviour — must add, update or delete its block in the
same change.** The full contract is [AGENTS.md](AGENTS.md); the format and
tooling are in [docs/INTERNAL-DOCS.md](docs/INTERNAL-DOCS.md).

```bash
cargo run -p kc-docs -- --write   # regenerate docs/internal (commit the result)
cargo run -p kc-docs -- --check   # what CI runs; fails on drift or bad blocks
cargo test -p kc-docs
```

`docs/internal/` is generated. Do not edit it by hand.

## Pull request checklist

* `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D
  warnings` pass, and `cargo fmt --all -- --check` is clean.
* Frontend changes: `npm test` and `npm run typecheck` pass in `web/`.
* `cargo run -p kc-docs -- --check` passes and `docs/internal/` is committed
  with the code change.
* `python3 scripts/check_readme.py` passes if you touched the README or the
  markdown it links.
* `python3 tests/test_build_site.py` passes, and the site builds cleanly, if you
  touched `docs/`, `mkdocs.yml` or the README's links:
  `python3 scripts/build_site.py && KARUCODE_SITE_DOCS=.site-docs mkdocs build --strict`
  (CI's `site` job runs exactly that).
* Every release is a commit on `main` (the version is the commit count), so the
  change should be done when it lands: describe what changed and how it was
  verified, and record user-visible changes in `CHANGELOG.md` under
  `[Unreleased]`.

## Reporting

Bugs and feature requests go to the issue tracker; security reports follow
[SECURITY.md](SECURITY.md). When reporting a bug, include the output of
`karucode --version` and the smallest reproduction you can manage.
