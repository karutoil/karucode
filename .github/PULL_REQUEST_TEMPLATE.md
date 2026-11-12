<!-- The gates below are the same ones CI runs; green locally means green there. -->

## What changed

<!-- One or two sentences: the behaviour change, not the file list. -->

## Verification

<!-- Commands you ran and what they printed. A claim without its command is a
     description, not a verification. -->

- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo fmt --all -- --check` is clean
- [ ] `cargo run -p kc-docs -- --check` passes, and `docs/internal/` is committed
      with the code change (or no user-visible behaviour changed)
- [ ] Frontend touched: `npm test` and `npm run typecheck` pass in `web/`
- [ ] README or linked markdown touched: `python3 scripts/check_readme.py` passes

## Documentation

<!-- If a user could ask about it, the @kc-doc block was updated in the same
     change (see AGENTS.md). Say which topic slug, or why nothing was needed. -->

- [ ] The matching `@kc-doc` block is updated in this change

## Changelog

- [ ] User-visible changes are recorded in `CHANGELOG.md` under `[Unreleased]`
