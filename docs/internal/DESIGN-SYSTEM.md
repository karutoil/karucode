# The karucode web design system contract

This document is the authority for visual work on `web/`. A change that breaks a
rule here is a defect even if it looks fine on one screen.

It exists because the stylesheet drifted: three competing colour families, a
type scale with 25 steps (220 declarations below 11px), 20 corner radii and 285
distinct spacing values. That is what "washed out and clashing" means in
practice, and it is measurable — see *Diagnosis* below.

---

## 1. Diagnosis (measured, not opinion)

Taken from the running UI at 1440×960, Ember (default) dark, and from
`node web/scripts/design-audit.mjs`.

### 1.1 Three clashing colour families on one screen

Opaque background colours actually painted, with HSL saturation:

| Colour    | Role                | H   | S   | L   |
| --------- | ------------------- | --- | --- | --- |
| `#111213` | canvas              | 210 | 6%  | 7%  |
| `#141516` | project rail        | 210 | 5%  | 8%  |
| `#171819` | inspector           | 210 | 4%  | 9%  |
| `#1c1d1f` | cards/panels        | 220 | 5%  | 12% |
| `#222326` | elevated surface    | 225 | 6%  | 14% |
| `#13181f` | transcript card     | 215 | **24%** | 10% |
| `#1b212a` | preview/input       | 216 | **22%** | 14% |
| `#1f2630` | inset card          | 215 | **22%** | 15% |
| `#252c37` | avatar/chip         | 217 | **20%** | 18% |
| `#3d2f1d` | accent tint         | 34  | 36% | 18% |
| `#3a2b1d` | accent tint hover   | 29  | 33% | 17% |

Read the `S` column. The shell chrome sits at 4–6% saturation (essentially
neutral grey). The "fidelity role" surfaces — every card, input, inset, avatar
and transcript block — sit at **20–24%**, four times as saturated, and all of
them lean blue. The accent is a warm amber at `h=34`.

So a single screen is composed from a neutral-grey system, a blue-slate system
and a warm-amber system. Nothing agrees with anything. **This is the clash.**

Root cause: `web/src/theme/roles.ts` holds the v3 mock's literal hexes in
`ROLES[].ember` and, for `id === 'ember'`, returns them verbatim
(`emberValue()`). Those literals are blue-slate neutrals, while Ember's palette
`--theme-bg` etc. are warm-neutral. `devValue()` never runs for Ember, so the
incoherence is baked into the default theme and only the *other* themes get a
palette-derived answer.

### 1.2 The elevation ladder is too shallow to see

The five shell surfaces span `L` 7% → 14%. Adjacent steps are 1–2 points, which
at `S≈5%` is 2–4 RGB units — below the perceptual threshold, especially on the
many 8-bit panels people actually use. The UI reads as one flat field with some
rectangles; there is no readable sense of "this is above that".

### 1.3 Text contrast fails WCAG AA broadly

85 text nodes on the default screen; **30 of them are below 4.5:1**, and they are
the smallest type on screen:

| Text                     | Size  | Ratio |
| ------------------------ | ----- | ----- |
| `Ctrl N` shortcut        | 9.5px | 3.14  |
| session meta (`1 turn`)  | 9px   | 3.41  |
| path hint                | 10px  | 3.41  |
| `Today` group label      | 9px   | 3.41  |
| composer hints           | 9.5px | 3.49  |
| message time             | 10px  | 3.49  |
| date marker              | 10px  | 3.49  |
| "Ready" status           | 9.5px | 3.49  |
| preview empty state      | 11px  | 3.52  |
| inspector subtitle       | 10px  | 3.87  |
| provider chip            | 10px  | 3.98  |
| `Rewind` keycap          | 10px  | 4.08  |

Tiny type that also cannot be read is the second half of "washed out".

### 1.4 The scale is not a scale

`node web/scripts/design-audit.mjs`:

```
  414  font-size declarations outside the --fs-* tokens (25 distinct values)
  172  border-radius declarations outside the radius tokens (20 distinct values)
 1092  padding/margin/gap declarations outside the --space-* tokens (285 distinct values)
   15  raw colours and border colours outside the token layer
```

220 of the 414 font sizes are below 11px. The token layer publishes 7 spacing
steps; the components use 285 values.

### 1.5 A second, contradictory status palette

`todo.css` hardcodes `var(--warn, #d09a3c)` and `var(--ok, #3f9d6a)`. Both
tokens exist, so the fallbacks are dead — and the fallback colours contradict
the real ones (`--warn` is `#f1c691`, `--success` is `#88baa0`). A component
inventing its own reds and greens is the same defect as §1.1 at a smaller scale.

---

## 2. The palette model

One hue family per theme. The shell is **neutral**, tinted at most 6% toward
the theme's accent. No component may introduce a second hue.

- Surfaces carry the theme's neutral character, `S ≤ 8%`.
- Text is neutral, warmed or cooled a maximum of 16% saturation.
- Status colours (`success`, `warn`, `danger`, `info`) are the only saturated
  hues besides the accent, and they appear only on status, never on chrome.
- The accent is reserved for: the primary action, selection, focus, and the one
  active-state tint. Nowhere else.

Ember's identity is **warm amber accent on warm-neutral graphite**. Its greys
must lean warm (hue ≈ 30–40, `S ≈ 4–5%`), not blue (hue 210+).

---

## 3. Tokens

### 3.1 Surfaces — a documented ladder

Each step must be at least **3 L points** from its neighbour so it survives an
8-bit panel. The ladder is authored in `tokens.css` and every value derives from
`--theme-*`; the role layer must never reintroduce a literal.

| Token             | Ember      | L   | Use                                  |
| ----------------- | ---------- | --- | ------------------------------------ |
| `--theme-bg`      | `#0e0d0c`  | 5%  | app canvas, transcript               |
| `--theme-rail`    | `#131211`  | 7%  | far-left project rail                |
| `--theme-sidebar` | `#171514`  | 8%  | session list                         |
| `--theme-inspector` | `#161413` | 8%  | right panel                          |
| `--theme-surface` | `#1d1a18`  | 11% | cards, inputs, panels                |
| `--theme-raised`  | `#262220`  | 14% | menus, elevated cards                |
| `--theme-hover`   | `#302b28`  | 17% | hovered rows, pressed controls       |

### 3.2 Spacing — 10 steps, 4px rhythm

```
--space-1: 2px    --space-2: 4px    --space-3: 6px    --space-4: 8px
--space-5: 10px   --space-6: 12px   --space-7: 16px   --space-8: 20px
--space-9: 24px   --space-10: 32px
```

Snap an existing literal to the **nearest** step. Maximum movement is 2px, so no
layout may reflow. A value that genuinely needs to be off-scale (a 1px hairline,
a 33px optical alignment) must be a named token, not a literal.

### 3.3 Type — 11 steps, 10px floor, 11px floor for prose

```
--fs-micro: 10px     uppercase kickers and badges only; must be ≥ 4.5:1
--fs-tiny: 11px      dense metadata
--fs-small: 11.5px   labels, buttons, inputs
--fs-label: 12px     section labels, tab text
--fs-message: 12.5px message body
--fs-body: 13px      UI body
--fs-lead: 14px      dialog and card titles
--fs-title: 16px     page titles
--fs-heading: 18px   section headings
--fs-display: 20px   empty-state titles
--fs-hero: 24px      the one hero number
```

Rules:

- No text below **11px** except `--fs-micro`, which must be uppercase with
  letter-spacing and pass 4.5:1. `9px` and `9.5px` are gone.
- Uppercase kickers: `10px / 720 weight / 0.125em` — one definition, `.eyebrow`.
- Weights come from `{400, 500, 550, 600, 660, 700, 720, 800}`. No `430`, `480`,
  `650` one-offs.

### 3.4 Radius — 6 values

```
--radius-xs: 4px    --radius-sm: 6px    --radius-md: 8px
--radius: 10px      --radius-lg: 14px   --radius-pill: 999px
```

`50%` is allowed only for circles. `--radius-input` and `--radius-prompt` are
aliases, not new values.

### 3.5 Lines

Three tokens only: `--line-soft`, `--line`, `--line-strong`. A border colour is
never authored by hand, and a component never gets a fourth line tone.

---

## 4. Contrast floors (enforced by `src/theme/contrast.test.ts`)

| Role                                    | Floor  |
| --------------------------------------- | ------ |
| `--text` on any surface                 | 12.0:1 |
| `--text-secondary` on any surface       | 7.0:1  |
| `--muted` on any surface                | 4.5:1  |
| `--faint` — decorative only             | 3.0:1  |
| accent as text (`accentText`)           | 4.5:1  |
| ink on an accent fill (`on-accent`)     | 4.5:1  |
| focus ring against its surface          | 3.0:1  |

`--faint` may only be used for non-essential decoration (a divider glyph, a
drag handle). Never for text a user must read.

Every built-in theme, in its own mode, must satisfy these. A theme that fails is
a bug in `roles.ts` or in the palette, not a reason to lower the floor.

---

## 5. Rules for component CSS

1. No raw colour, radius, font-size or spacing literal. Use a token.
2. No new `--theme-*` property outside `tokens.css`.
3. A component may not define a hue the palette does not have.
4. Hover/focus/active states come from `--surface-hover`, `--accent-bg`,
   `--line-strong` — not from hand-mixed shades.
5. Depth comes from the ladder in §3.1 plus the two shadow tokens
   (`--shadow-menu`, `--shadow-overlay`). Do not author a third shadow.
6. Every interactive element needs a visible focus ring using
   `--focus-ring` at ≥ 3:1 against its surface.
7. Motion uses `--transition` / `--transition-fast`, and must be wrapped in
   `@media (prefers-reduced-motion: reduce)` when it is longer than a fade.

---

## 6. How this is verified

- `node web/scripts/design-audit.mjs` — static: token-rule violations with
  `file:line`. The target is **zero** outside `tokens.css`.
- `cd web && npx vitest run` — `contrast.test.ts` for §4, `roles.test.ts` for
  palette derivation, and the per-file style contracts for structure.
- A real Chromium over CDP, driving every screen in dark and light mode. jsdom
  cannot see a contrast ratio, an overlap or a clipped row; a screenshot can.
  Evidence lands in `docs/internal/design-verify/`.