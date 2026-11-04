# The design audit

**Status:** findings from four parallel domain reviews, plus the measured
diagnosis that produced them. The contract they are judged against is
[`DESIGN-SYSTEM.md`](./DESIGN-SYSTEM.md).

The complaint was *"the UI feels washed out, the designs are clashing, and it can
be a lot better."* That is three separate defects and each one turned out to be
measurable.

---

## 0. What was actually wrong

| Symptom | Measurement | Root cause | Status |
| --- | --- | --- | --- |
| "Clashing" | Shell chrome at **4–6% saturation** beside card/input/inset fills at **20–24%**, all leaning blue, under a warm amber accent | `roles.ts` returned the v3 mock's *literal* hex values for `id === 'ember'`; every other theme derived from its palette | **fixed** — all themes derive |
| "Clashing" | Ember's own palette described itself as "warm carbon" but `#111213` is `h≈210` (cool) | palette authored by hand against a mock | **fixed** — `h≈32` |
| "Washed out" | **30 of 85** text nodes below WCAG AA; the worst were the *smallest* (9px/9.5px at 3.1–3.5:1) | nothing enforced a contrast floor | **fixed at token layer** — 0 of ~90 |
| "Washed out" | Five shell surfaces spanning L 7%→14% in 1–2 point steps (2–4 RGB units) | no elevation model | **fixed** — steps ≥3.5 points |
| Incoherent | **414** font-size (25 distinct, 220 below 11px), **172** radii (20 distinct), **1092** spacing (285 distinct) | the three documented token rules were never enforced | **in progress** — `scale.test.ts` now enforces |

---

## 1. The four domain reviews

Each file is a standalone review with `file:line` citations, quoted
declarations, measured numbers and a token-based fix per finding.

### [`shell.md`](./design-audit/shell.md) — 312 lines · 3 blockers, 7 major, 12 minor
App shell, header, workspace tabs, project rail, sessions sidebar, boot/empty/
auth screens. Confirms §1.1's clash no longer reproduces in the shell and
re-baselines against the new palette. Surviving defects are structural:
- Light mode fails because **15 text roles use `--faint`**, which §4 reserves for
  decoration — 20 of 34 shell text nodes below 4.5:1, worst 2.38:1 at 9px.
- Two opacity multipliers (`.8`, `.7`) cost a further 1.6–2.9 contrast points.
- 260 token violations in its three files (50 sizes, 27 radii, 183 spacings).

### [`transcript.md`](./design-audit/transcript.md) — 371 lines · 3 blockers
Transcript, message rows, tool cards, tool groups, diff view, composer dock.
- **Dead custom properties:** `--surface-2`/`--surface-3` pointed at
  `--raised`/`--hover`, which never existed, so **seven fills painted
  `rgba(0,0,0,0)`** (avatar, status chip, tool-group header, disabled Send, four
  hover states). *Fixed in the token layer during this audit.*
- **Light mode is the failure:** 73 of 168 text nodes below AA.
- Two intended reading-size rules are dead selectors, so answer, narration and
  user text render at 12.5 / 12.5 / 12.8px — the hierarchy is invisible.
- Card interiors run **backwards**: card L11 → code L9 → diff L8, with L16
  headers/footers. Light-mode ladder collapses to 1 lightness point (1.02:1).
- Diff rows are **46–60% saturated** surfaces: 2.88:1 (add) / 2.81:1 (del) in
  light — the surface a user reads before approving a write.
- Composer focus ring is `outline: none` + a 1px border = **2.27:1** in light.
- 124 font-size declarations, 82 literals; 0 of 199 spacings use `--space-*`.

### [`overlays.md`](./design-audit/overlays.md) — 370 lines · 6 blockers, 8 major, 13 minor
Dialog, command palette, toasts, rewind, mention menu, notifications, the
settings sheet and the Theme Studio.
- These four files hold **445 of 1693** token violations from 24.4% of CSS lines.
- **Two scrims, both literal black, neither inverts:** `color-mix(#000 67%)` with
  `blur(4px)` vs `var(--scrim)` at 60% with `blur(3px)`. In Daybreak the canvas
  drops from L95.5% to L31.5%.
- **Accent used as ink and `--faint` used as text:** active palette item 3.61:1,
  theme eyebrow 3.80:1, primary button ink 4.00:1, palette Esc 2.91:1 — while
  `--accent-text` (5.31:1) and `--accent-fill`/`--on-accent` (5.83:1) already pass.
- Settings information architecture: three section rhythms (43/36/29px), control
  heights 37/32/30/16 and right edges 1427/1113/985 — nothing aligns.
- `.theme-studio-layer { z-index: 110 }` is **inert** (not portalled, inside
  `isolation: isolate`), leaving two `aria-modal="true"` surfaces alive.
- Ten line tones where §3.5 allows three; 36 of 68 radii off-scale.

### [`panels.md`](./design-audit/panels.md) — 322 lines · 3 blockers
Inspector, file tree/viewer, session info, agents, control plane, terminal, git,
todo, goals.
- **Elevation is unreadable:** adjacent cards 1.00–1.23:1; `--surface-inset` is
  now *brighter* than `--theme-surface` and 1.01:1 from `--theme-raised`.
- **Git panel is functionally broken at 342px:** a 1241px diff line renders in a
  238px box with `overflow: hidden`; `.diff-lines`' `overflow-x: auto` never
  fires; the file header gets 81.5px against 175.5px of buttons.
- **Three escaping palettes:** todo's 13 literal `#d09a3c`/`#3f9d6a`; session
  control's 12 raw colours; **terminal ANSI blue renders amber** (`--accent`)
  and ANSI 0 at 2.08:1.
- Goals/agent progress tracks at 1.01:1 and 1.50:1; the tree indent formula
  reaches 584px inside a 293px row.

---

## 2. Cross-cutting findings (not any one file's problem)

1. **Light mode is where the design fails.** Every reviewer independently found
   that dark mode is now largely sound while Daybreak does not meet AA. The
   palette roles pass their floors; the *components* then paint `--faint` and
   `--accent` as text, apply opacity to already-quiet greys, and re-introduce
   raw literals. Fixing the token layer was necessary and not sufficient.
2. **Undefined custom properties are silent.** `--surface-2`, `--surface-3`,
   `--warning`, `--mono` and `--focus-ring` were all read and never defined, so
   declarations became invalid at computed-value time with no error. Fixed for
   the five above; a component may not invent a token name.
3. **The tests asserted the drift.** `geometry.test.ts` and `typography.test.ts`
   pinned the mock's 11px/13px radii, 33px margins and 25 font sizes, so every
   correction was a test failure. Both are now scale contracts instead.
4. **Nothing enforced the three rules `tokens.css` documents.** That is now
   `scale.test.ts`, which fails on any raw colour, off-scale radius, sub-11px
   font size or off-rhythm spacing outside the token layer.

---

## 3. The plan

| Loop | Who | What |
| --- | --- | --- |
| 1 | 4 reviewers | This document and the four domain files. **done** |
| foundation | coordinator | `roles.ts` derives for all themes; palettes snapped to a visible ladder; contrast floors enforced in `resolvePalette`; `tokens.css` generated from the resolver with a drift guard; `scale.test.ts` + `contrast.test.ts`. **done** |
| 2 | 4 workers | One per domain: migrate every literal onto the scale, delete the escaping palettes, fix the light-mode contrast failures and the focus rings, update the style tests that pinned the old values. |
| 3 | 4 reviewers | Re-measure the same screens in a real browser, dark and light, and report what the workers broke or missed. |
| 4 | as needed | Fix the review findings; final verification. |

Success is not "it looks better". It is: `scale.test.ts` at zero violations,
`contrast.test.ts` green for all eight themes, no text below 4.5:1 in a real
browser in either mode, and every screen driven end to end.
---

## 4. Outcome (measured, not asserted)

| | Before | After |
| --- | --- | --- |
| Text below WCAG AA on the default screen | **30 of 85** nodes (worst 3.14:1 at 9.5px) | **0 of 52** in Ember dark, **0 of 52** in Daybreak light |
| Settings and command palette | — | 0 of 85 and 0 of 70, both modes |
| Raw colours outside the token layer | 15 | **0** |
| Corner radii off the scale | 172 (20 distinct) | **0** |
| Hand-written font sizes | 414 (25 distinct, 220 below 11px) | **0** below the 11px floor |
| Spacing values off the rhythm | 1092 (285 distinct) | **0** |
| Surface hue families on one screen | 2 (neutral chrome at 5% saturation, blue-slate cards at 22%) | **1** (warm, `h≈32`, ≤7% saturation) |
| Adjacent elevation steps | 1–2 lightness points (2–4 RGB units) | **≥3.5 points** |
| `npx tsc --noEmit` | 17 errors | **0** |
| Web test suite | 1331 passing | **1470 passing**, 0 failing |

Reproduce any row:

```
cd web && npx vitest run src/theme/contrast.test.ts     # the §4 floors, all 8 themes
cd web && npx vitest run src/styles/scale.test.ts       # the §3 token rules
node web/scripts/design-audit.mjs                        # human-readable violation list
python3 scripts/design_verify.py --base <live-server>    # real Chromium, both modes
```

### What is deliberately not claimed

- **`artifacts.css` is exempt and is being migrated by its own owner.** The
  `scale.test.ts` allowlist names it and asserts the list stays short; it is
  expected to go clean and the exception removed.
- **A few text-derived roles still sit below `--muted`.** `--text-empty-copy` was
  one, measured at 4.2:1 in Daybreak, and it is now overridden at its one use site
  rather than re-derived. The systemic fix — bounding every `--text-*` role by the
  muted floor inside `roles.ts` — is the right next step and is not done.
- **The verification covers six screens in two modes at three widths.** Terminal,
  git diff, todo and goals were reviewed by their owners in a browser but are not
  in the automated matrix yet.
