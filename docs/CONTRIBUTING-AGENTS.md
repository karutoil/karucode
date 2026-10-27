# Shared build rules (read before editing)

karucode is a Rust workspace plus a Preact frontend. Multiple agents work in this
checkout at the same time. Follow these rules strictly.

## Ownership

Only edit files inside the paths you were assigned. If you need a change in
another crate, send a message to the lead (`orchestrator`) describing the exact
signature you need. Never rewrite another agent's crate.

## Frozen interfaces

`crates/kc-core` defines the contracts. Do not change public signatures there
without asking the lead. Adding new public items is fine when additive, but say so
in your completion message.

If a `todo!()` in another crate blocks your manual testing, do not implement it:
use a fake in your own tests.

## Toolchain

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo check -p <your-crate>
cargo test -p <your-crate>
cargo clippy -p <your-crate> -- -D warnings
cargo fmt -p <your-crate>
```

Rust 1.96, edition 2021. Dependencies come from `[workspace.dependencies]`; add a
new dependency to the workspace table only if necessary, and prefer none.

## Quality bar

* No placeholder or fake implementations in the code you deliver. `unimplemented!`/
  `todo!()` may remain only where the lead explicitly said a stub is acceptable.
* Every module you implement needs unit tests that run with `cargo test -p ...`.
* Errors must be actionable: include path/command/exit code/status, never leak
  secrets (API keys, tokens, headers) into messages or logs.
* Keep dependencies small. This product is judged on idle RSS, startup time and
  responsiveness.
* Format with `cargo fmt`, keep clippy clean for your crate.
* Write comments only where the reasoning is non-obvious.

## Reporting

When done, send the lead a completion message with: files created/changed, public
API you added, tests you ran with results, anything you could not verify, and any
integration requirement the server must satisfy.
