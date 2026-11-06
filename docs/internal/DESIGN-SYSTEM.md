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

### 2.1 An achromatic anchor lends the canvas tint

Every role inherits the hue and saturation of the palette colour it is cut from.
The exception is an anchor that has no tint to give: every light built-in's
`surface` is pure `#ffffff`, so a card cut from it once came out a *neutral grey*
(`#f6f6f6`, `h=0 s=0`) on a warm canvas (`#f8f5ef`, `h≈40`) — a second, colourless
design system in the same screen, which is the light-mode half of "the designs
clash".

`tintFor()` in `roles.ts` therefore falls back to the theme's own chromatic
surface (canvas first) when the anchor's saturation is below 6%. `contrast.test.ts`
measures each role against *that* colour, so the guard now covers the roles it
used to skip and a near-grey role is judged by its RGB channel spread (HSL
saturation is numerically meaningless at the ends of the ramp).

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

**Light themes invert this.** The palette's `surface` is pure white and the canvas
is off-white, so the ramp *above* the canvas is at most 1.05:1 wide and a mirrored
offset lands a card in the dead centre — measured `#f6f6f6` on a `#f8f5ef` canvas,
1.02:1, lighter than the ground it sits on. `liftOffCanvas()` in `roles.ts` snaps a
fill that lands above the canvas but within 1.03:1 of it up to its anchor's own rung
(now `#fcfcfb`, 1.07:1 in Daybreak). A *well* — console, code block, diff body —
lands below the canvas in light mode and is left there: a recess must not be lifted
into a second card.

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

### 4.1 Every ink role is floored against every surface

A role named `--text-*`, `--input-ink`, `--input-placeholder`, `--foot-*`,
`--chip-ink`, `--meta-*` — plus the accent-named inks `--tab-active-text`,
`--tab-hover`, `--row-selected-text`, `--row-accent-ink`, `--rail-glyph-text`,
`--rail-avatar-text` and `--toggle-knob` — is bounded at **4.5:1 against the
surface nearest to it**: the lightest surface in a dark theme, the darkest in a
light one (`applyInkFloor()` in `roles.ts`). Roles cut from `faint` were the ones
that suffered: `faint` is decorative (3:1), so `--text-subtle` inherited "quiet"
down to 2.95:1 on Daybreak's raised surface — text a user must read, in the least
readable state the app has.

Icon roles are deliberately *not* floored: brightening a drag handle is noise, not
legibility. The floor outranks the hierarchy guard that keeps secondary text under
primary text, because an unreadable rung is not a hierarchy.

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
8. Accent ink on an accent tint is `--accent-on-tint`, never `--accent`,
   `--accent-fill` or `--accent-text`: those are tuned against the canvas, and a
   tint lifts the ground under them (measured 3.62:1 in Daybreak). `accent.test.ts`
   fails on any rule that paints one of the accent tints and then inks it with a
   canvas-tuned accent.
9. A control row that can be narrower than its contents must let its flex items
   shrink (`min-width: 0`) and ellipsise; `overflow: hidden` on the parent is not
   permission to lose a control. `.compose-bar-left` clipped the reasoning pill at
   1024px until `.model-switcher-button` gained `min-width: 0` and
   `max-width: min(230px, 100%)`.

---

## 5A. One product, one vocabulary: the control plane parity rule

The control plane was built as its own screen and grew a **private** vocabulary:
112 `.cp-*` classes re-drawing rows, buttons, badges, agent cards, facts, empty
states and a rail head/foot that the workbench already has as shared primitives.
It is token-clean (§6 reports 0 colour violations), so the defect is not palette
drift — it is that the same *role* is drawn twice, differently. Measured at
1440×900 in Ember, a session row in the rail and the same session in the control
plane disagree on title weight (500 vs 600), meta font (Inter vs mono), meta
line-height (16.5px vs 11px), status treatment (dot + label vs label alone),
row padding and height. A user flipping between the two sees two products.

**The rule: a control-plane element that plays a role the workbench already has
must use the workbench's class for that role.** `.cp-*` may remain only for what
is genuinely control-plane-specific — the two-group rail, the launch box, the
overview dashboard grid, the `k.` chat glyph.

The mapping is the contract. Left is the private class, right is the shared one
to adopt (owner file in parentheses):

| Role | Private | Shared |
| --- | --- | --- |
| default button | `.cp-secondary` | `.button` (base.css) |
| primary button | `.cp-primary` | `.button .button-primary` (base.css) |
| small button | `.cp-small` | `.button .button-small` (base.css) |
| destructive button | — | `.button-danger` / `.button-danger-ghost` (base.css) |
| icon-only button | `.cp-icon`, `.cp-back` | `.icon-button` (base.css) |
| text link | `.cp-link` | `.link-button` (base.css) |
| filter pill | `.cp-filters button` | `.filter-chip` (+`.is-on`) (layout.css) |
| search field | `.cp-search input` | `.side-search` / `.input` (layout.css, base.css) |
| count badge | `.cp-badge`, `.cp-badge-agents` | `.badge` (+`-ok/-warn/-danger/-muted`) (base.css) |
| status indicator | `.cp-state i`, `.cp-dot` | `.status-dot .status-<tone>` (base.css) |
| section kicker | `.cp-eyebrow` | `.eyebrow` (base.css) |
| session row | `.cp-row*` | `.session-row`/`.session-open`/`.session-head`/`.session-title`/`.session-meta`/`.session-state-<tone>` (layout.css) |
| row kebab + menu | `.cp-row-menu*` | `.session-menu-wrap`/`.session-menu-trigger`/`.session-menu`/`.menu-item` (layout.css) |
| rail head / foot | `.cp-browser-head`, `.cp-browser-foot` | `.side-head`, `.side-footer`/`.online-row` (layout.css) |
| agent card | `.cp-agent*` | `.agent-card`/`.agent-head`/`.agent-name`/`.agent-role`/`.agent-task`/`.agent-note`/`.agent-progress` (inspector.css) |
| facts / metrics | `.cp-facts dl` | `.inspector-section`/`.inspector-kicker`/`.metric`/`.metric-label`/`.metric-num` (inspector.css) |
| empty state | `.cp-intro*` | `.empty-state`/`.empty-symbol`/`.empty-title`/`.empty-copy` (chat.css) |
| brand mark | `.cp-brand-mark` | `.brand-mark` (rail.css) |
| project avatar | `.cp-glyph` | `.rail-avatar` / `.project-rail-avatar` (rail.css) |
| reason options | `.cp-launch-pick` (own copy) | `REASONING_OPTIONS` from `ModelSwitcher.tsx` — one list, not two |

Same for logic, not just CSS. `shortAgo` → `relativeTime` (`lib/format.ts`);
`initials` → `projectInitials` (`ProjectRail.tsx`); the inline
`status === 'running' || status === 'paused'` → `isAgentLive` (`api/types.ts`);
`agentStatusWord` → the exported agent status map; `sessionStatus` → the shared
session-status view in `state/selectors.ts`. **One definition per fact**: if the
workbench and the control plane can disagree about how "3 minutes ago" or
"completed" is spelled, the helper is in the wrong place.

Glyphs are not a vocabulary. `⟳ ⌕ ＋ → ✳ ↗ ←` are text characters with no
consistent metrics, weight or baseline; `components/icons.tsx` publishes 51 icons
that the rest of the app uses. Every hand-rolled glyph in the control plane
becomes an icon component (add `ExternalIcon` for "open elsewhere" — it is the
one role genuinely missing).

**Ink must match the role, not the screen.** The control plane inks row titles
with `--text` where the workbench uses `--text-secondary`, and kickers with
`--muted` where the workbench uses `--text-kicker`. Individually invisible;
together they make the plane read a step brighter. Use the same ink token for the
same role.

**Responsive strategy must match too.** The workbench answers its own width with
container queries (`container-type: inline-size`, `container-name: workbench`,
breakpoints at 780/1000/1100px). The control plane uses 13 viewport `@media`
breakpoints, so at 1280px the workbench is still two-up while the plane has
already stacked its inspector under the composer. A screen embedded in the same
window must respond to the same width the workbench responds to: declare
`container-type: inline-size` on `.control-plane.cp` and convert the layout
breakpoints to `@container`. Phone-only concerns (`dvh`, safe-area) may stay
`@media`.

Finally, the plane's own header must not restate what the embedded conversation
already says. `.cp-session-head` and the `ChatView` header both render the title
and run state; pick one owner (§ shared rule 5, "one owner per fact").

---

## 6. How this is verified

- `node web/scripts/design-audit.mjs` — static: token-rule violations with
  `file:line`. The target is **zero** outside `tokens.css`.
- `cd web && npx vitest run` — `contrast.test.ts` for §4, `roles.test.ts` for
  palette derivation, and the per-file style contracts for structure.
- A real Chromium over CDP, driving every screen in dark and light mode. jsdom
  cannot see a contrast ratio, an overlap or a clipped row; a screenshot can.
  Evidence lands in `docs/internal/design-verify/`.