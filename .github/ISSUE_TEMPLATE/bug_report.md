name: Bug report
description: Something does not work the way this repository says it does.
labels: ["bug"]
body:
  - type: markdown
    attributes:
      value: |
        A reproduction we can run beats a description we have to imagine. The
        fastest path is `examples/demo-app` (a dependency-free Node project with
        a deliberate bug) or a two-line workspace.
  - type: input
    id: version
    attributes:
      label: karucode version
      description: Output of `karucode --version` (the build is the commit count).
      placeholder: "karucode 107 (abf0a4216839, 2026-11-09T21:35:10-05:00)"
    validations:
      required: true
  - type: dropdown
    id: os
    attributes:
      label: OS
      options:
        - Linux
        - macOS
        - Other
    validations:
      required: true
  - type: input
    id: provider
    attributes:
      label: Provider / model
      description: Which endpoint and model (gateway name, model id). Redact keys.
      placeholder: "LiteLLM gateway / claude-sonnet"
  - type: textarea
    id: repro
    attributes:
      label: Steps to reproduce
      description: The exact commands or UI steps, and the workspace contents if it matters.
      placeholder: |
        1. cargo run --release -p kc-server -- --workspace examples/demo-app
        2. prompt: "run the tests and fix the bug"
        3. ...
    validations:
      required: true
  - type: textarea
    id: expected
    attributes:
      label: What you expected
      placeholder: The run re-runs the tests and they pass.
    validations:
      required: true
  - type: textarea
    id: actual
    attributes:
      label: What happened instead
      description: Error text, event log excerpts, `reports/e2e.json` output — anything concrete. Redact secrets.
    validations:
      required: true
  - type: textarea
    id: config
    attributes:
      label: Relevant configuration
      description: config.toml / environment excerpt, approval mode, limits. Redact keys and tokens.
      render: toml
