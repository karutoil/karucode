# Demo app

A deliberately buggy Node.js project for trying karucode end to end. No
dependencies: `node --test` is enough.

```bash
cd examples/demo-app
node --test          # 1 failing test: add(2, 3) returns -1 because of a typo
```

Try a prompt such as:

> Run the tests, find the failing case, fix the bug in src/math.js, and re-run the
> tests to prove it passes.

The agent should read `src/math.js`, run `node --test`, change the operator, and
re-run the suite. With the default `ask` approval mode you will be asked before
the command runs — approve it in the UI and watch the streamed output.

Suggested follow-up prompts:

* "Show me the diff and explain the fix in one sentence."
* "Add a `sum` helper with tests, then run the suite."
* "Delegate a review of the change to a subagent with a different model."
