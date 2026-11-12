name: Feature request
description: Something karucode should do that it does not.
labels: ["enhancement"]
body:
  - type: textarea
    id: problem
    attributes:
      label: The problem
      description: What you were trying to get done when the tool fell short — not the solution yet.
    validations:
      required: true
  - type: textarea
    id: proposal
    attributes:
      label: What you would want instead
      description: The behaviour you expect, in one or two sentences. A config key, a UI change, a new tool — say which.
    validations:
      required: true
  - type: textarea
    id: alternatives
    attributes:
      label: Alternatives you considered
      description: Workarounds you use today, other tools that do it, or why none of those fit.
