# Conversation surface — transcript and composer

Audit of `web/src/styles/chat.css` and `web/src/styles/composer.css` and the
components that paint them (`ChatView`, `ChatTranscript`, `Composer`, `ToolCard`,
`ToolGroup`, `ThinkingBlock`, `DiffView`, `Markdown`, `TpsBadge`, `CommandCard`),
against `docs/internal/DESIGN-SYSTEM.md` (the contract; §numbers below are its
sections).

## Measurement basis

- Repo `/home/karutoil/karucode`, master `978631f` + uncommitted work.
- `chat.css` md5 `f0aa9949…`, `composer.css` md5 `b5006918…` — **unchanged for
  the whole audit**, so every finding below is reproducible from those two files.
- The token layer changed underneath the audit: `tokens.css` was rewritten at
  11:51–11:53 and `web/dist` rebuilt at 11:53:45. Measurements are labelled
  **pre-fix build** (`dist` 11:35, the build `DESIGN-SYSTEM.md` §1 diagnoses) and
  **current build** (`dist` 11:53). The component CSS is the same in both.
- Chromium over CDP at 1440×960, Ember dark and Daybreak light, session
  "Design review seed" driven through `POST /api/sessions/:id/messages` with the
  mock provider (`[tool:multi]`, `[tool:run]`, `[tool:write]`, `[tool:bad]`), plus
  the Theme Studio's `grouping=tools` to render a real `ToolGroup`.
- Text nodes were counted as elements with direct text content inside
  `.transcript` + `.composer-wrap`; backgrounds were composited through every
  ancestor with alpha; ratios are WCAG 2.x relative luminance.

## Verdict

In Ember dark the conversation surface is now the healthiest part of the app —
**1 of 164 text nodes is below 4.5:1** — but that is the token layer's doing, not
this surface's: the same build before the token rewrite measured **37 of 112**
below AA, with every message timestamp at 3.49:1. In Daybreak light the surface
is the worst: **73 of 168 text nodes below 4.5:1**, including the tool output body
(4.27:1) and every tool line number (3.15:1), and the diff — the one thing a
reader must read before approving a change — at 2.88:1 / 2.81:1. Underneath the
palette, the component layer has simply not adopted the contract: 124 `font-size`
declarations of which **82 are literals** (9px and 9.5px are still in use 20
times), **0 of 199** spacing declarations use `--space-*`, 62 radii of which **42
are literals**, 13 line variables for what §3.5 says is one job, two rules that were
supposed to set the reading size are dead, and two custom properties
(`--surface-2`, `--surface-3`) resolve to the empty string so seven declarations
paint nothing at all. The palette clash §1.1 documents is fixed in the
current tree; the *component* debt it was hiding is not.

## Surface inventory — the clash, measured

Every opaque surface on the conversation screen, with HSL and contrast against
the canvas. The pre-fix column is the build §1.1 was written from.

| Surface | pre-fix build | current build | light build (current) |
| --- | --- | --- | --- |
| canvas `--theme-bg` | `#111213` 210 **6%** 7% | `#0f0e0d` 30 7% 5% | `#f6f4f1` 36 22% 95% |
| rail / sidebar / inspector | 4–5% S | 4–5% S | 3–4% S |
| `--theme-surface` | `#1c1d1f` 220 5% 12% | `#23211f` 30 6% 13% | `#ffffff` 0 0% 100% |
| **transcript card** `--surface-card` | **`#13181f` 215 24% 10%** | `#1e1c1b` 20 5% 11% (1.14:1 vs canvas) | `#f6f6f6` 0 0% 96% (**1.02:1**) |
| composer/input `--surface-input` | `#1b212a` 216 **22%** 14% | `#272422` 24 7% 14% (1.25:1) | `#f7f7f7` 0 0% 97% (1.02:1) |
| inset / code head | `#1f2630` 215 22% 16% | `#2b2926` 36 6% 16% | `#f0f0f0` 0 0% 94% |
| code block `--code-bg` | `#171819` 210 4% 9% | `#181716` 30 4% 9% | `#f4f1eb` 40 29% 94% |
| diff body `--surface-diff` | `#0a0d11` 214 26% 5% | `#151312` 20 8% 8% | `#ececec` 0 0% 93% |
| diff add / del rows | `#0f2a20` 158 47% 11% / `#2e1a1a` 0 28% 14% | `#172920` 150 **28%** 13% / `#38150e` 10 **60%** 14% | `#0a281f` 162 **60%** 10% / `#3c1816` 3 **46%** 16% |
| card border `--line-card-strong` | `#2b333e` 215 18% 21% | `#433b30` 35 **17%** 23% (1.54:1 vs its own fill) | `#d1cbc0` 39 16% 79% (1.49:1) |
| line `--theme-line` | `#2b2d2f` 210 4% 18% | `#363430` 40 6% 20% | `#e6e2dc` 36 17% 88% |

Read the saturation column of the pre-fix build: the shell is 4–6% and every
"fidelity role" — card, input, inset, symbol, diff, card border — is 18–26% and
all of it leans blue, beside a warm amber accent at h=34. That is §1.1 exactly,
and the transcript card is the worst offender (24% against a 6% canvas).

In the current build that clash is gone from every *fill* — all shell and card
surfaces now sit at S 4–9%, hue 20–40 — so the remaining saturated colours on
the conversation screen are the two diff-row tints (28% and 60%) and the card
border (17%). Two consequences worth stating plainly:

- The diff row is the only surface that still breaks §2's `S ≤ 8%`, and it is
  the one that fails contrast in light mode (2.88:1 / 2.81:1).
- The card border is a *hue* change rather than a strength change: `#433b30`
  against `#363430` is 1.13:1, i.e. the "strong" line and the normal line are
  the same tone painted in two families.
- On the pre-fix build, creating an otherwise untouched Theme Studio *draft*
  changed the same card from `#13181f` (24% S) to `#171719` (HSL 240 **4%** 9%):
  the literal `emberValue()` path stopped running and `devValue()` derived the
  role. That is §1.1's root cause reproduced from the conversation surface — the
  card's colour depends on whether a draft exists, same DOM, same CSS.

## Blockers

- **B1 — `--surface-2` and `--surface-3` resolve to nothing; seven declarations
  paint nothing.**
  `tokens.css:334-335` — `--surface-2: var(--raised);` `--surface-3: var(--hover);`
  — but no `--raised` and no `--hover` exist anywhere in the token layer (the real
  names are `--surface-raised` / `--theme-raised` and `--surface-hover` /
  `--theme-hover`). Measured on the live page:
  `getComputedStyle(document.documentElement).getPropertyValue('--surface-2') === ''`
  (same for `--surface-3`, `--raised`, `--hover`), and every background below
  computes to `rgba(0, 0, 0, 0)`:
  - `chat.css:128` — `background: var(--surface-3);` — the 29×29 user avatar tile,
    measured `background-color: rgba(0, 0, 0, 0)`: it is a bare 1px outline.
  - `chat.css:303` — `background: var(--surface-2);` — the 17px `Completed` /
    `Working` chip, measured transparent.
  - `chat.css:1318` — `background: var(--surface-2);` — the tool-group header,
    measured transparent (with `grouping=tools`; the variable is empty in every
    state), so the group and its body are one flat block with a 1px border
    between them.
  - `composer.css:588` — `background: var(--surface-3);` — the disabled Send
    button: 28×28, `background-color: rgba(0, 0, 0, 0)`, glyph at 4.29:1. On an
    empty session the app's primary action is an invisible square.
  - `composer.css:450`, `:493`, `:665` — `background: var(--surface-2);` — the
    attach button, both model/reasoning pills and the context pill have **no hover
    state at all** (four of the composer's five hoverable controls; only Send
    responds).
  **Fix:** point the aliases at real ladder steps — `--surface-2: var(--surface-raised)`
  (§3.1 `--theme-raised`, L 14%) and `--surface-3: var(--surface-hover)`
  (§3.1 `--theme-hover`, L 17%) — or replace the seven uses with
  `--surface-raised` / `--surface-hover` directly. Add a unit assertion that no
  `var(--x)` used by a component resolves to the empty string.

- **B2 — Daybreak light fails AA across the conversation surface: 73 of 168 text
  nodes below 4.5:1.**
  Worst measured offenders (element, size, ratio):
  tool output body `chat.css:975-983` `color: var(--text-output)` at 11px —
  **4.27:1** (`#7e7460` on `#f6f6f6`); every tool line number
  `chat.css:1087-1092` `color: var(--text-output-num)` — **3.15:1**;
  `.msg-time` / `.date-marker` `chat.css:178, 72` `color: var(--faint)` at 10px —
  **3.62:1**; composer hints/footnote `composer.css:804-806, 831-832, 857-858,
  876-877` at 9.5px —
  **3.62:1**; approval kicker `chat.css:1618` `color: var(--accent)` at 10px —
  **4.06:1**; primary-button ink on `--accent-fill` — **4.0:1** (measured live;
  4.39:1 against pure white). §4 floors are `--muted` ≥ 4.5, `--text-secondary`
  ≥ 7.0, ink on accent ≥ 4.5.
  **Fix:** stop using `--faint`/`--text-output-num` for text (§4: "`--faint` may
  only be used for non-essential decoration"); metadata takes `--muted`
  (`--fs-tiny`, 11px), code/tool output takes `--text-secondary`
  (`--fs-tiny`), and `--on-accent` on `--accent-fill` must be measured per theme
  in `contrast.test.ts`. The light pair measures 4.39:1 against a 4.5 floor, and
  the live button 4.0:1, so this is a real failing pair, not a rounding edge.

- **B3 — diff rows are unreadable in light mode and are the only 46–60% saturated
  surfaces left.**
  `chat.css:1877` — `background: var(--surface-diff-add);` and `chat.css:1882` —
  `background: var(--surface-diff-del);` — in Daybreak those tokens are `#0a281f`
  (HSL 162 **60%** 10%) and `#3c1816` (HSL 3 **46%** 16%), i.e. a near-black
  rectangle in a 95%-lightness UI, while the ink flips to a mid-tone. Measured on
  the rendered `.diff-line`: add text **2.88:1**, del text **2.81:1**. In dark the
  same rows measure 6.97:1 / 7.49:1, but the del fill is only 1.04:1 from the card
  it sits in, so the tint carries nothing there either.
  **Fix:** build the row from §3.1 + §2: fill from the status *tint* steps
  (`--success-bg` `#293830`, `--danger-bg` `#42241f` — mode-aware, and retune
  `--danger-bg` from its present 36% S) and ink from `--success` / `--danger`;
  keep the surface itself on the ladder (`--theme-surface`/`--theme-raised`). Do
  not paint a surface with a colour whose saturation is 6× the shell's.

- **B4 — 9px and 9.5px type is still in use 20 times, on text a user must read.**
  §3.3: "9px and 9.5px are gone", floor 10px, and only for uppercase `--fs-micro`
  badges.
  `chat.css:334, 349, 360, 374` — `font-size: 9px;` on `.tag-live`, `.tag-failed`,
  `.tag-stopped`, `.model-pill` (the model id is 9px); `chat.css:821, 846, 858`
  — `font-size: 9px;` on the thinking disclosure glyph, the thinking preview and
  the "Show reasoning" hint; `chat.css:1566, 1633` — `font-size: 9.5px;` on the
  stdout/stderr labels and the approval kind chip; `composer.css:288` (queued
  steer/follow-up label), `:1144` (footnote at < 800px), `:191, 253, 336, 657,
  806, 832, 858, 877, 1172` (chip meta, queued head, attachments, context pill,
  footnote, hints, approvals) — `font-size: 9.5px;`.
  Measured: `.model-pill` renders 9px/15px in every assistant row; `.thinking-hint`
  9px; the whole composer footnote 9.5px at a 21px line box.
  **Fix:** `--fs-micro` (10px, uppercase + tracking only) for the status chips and
  labels, `--fs-tiny` (11px) for previews/hints/attachment meta, `--fs-label`
  (12px) for the model pill. Delete the 9px phone override at `composer.css:1144`
  — a footnote that has to shrink below the floor at 800px should be removed, not
  shrunk.

## Major

- **M1 — the reading size is set by two dead rules; every prose block renders at
  12.5px.**
  `chat.css:501-504` — `.msg-assistant .msg-body > .markdown { font-size: 13px;
  line-height: 1.64; }` never matches: `ChatTranscript.tsx:216` wraps every prose
  part in `<div class={narration ? 'narration' : undefined}>`, so `.markdown` is
  never a direct child of `.msg-body`. Measured:
  `document.querySelectorAll('.msg-assistant .msg-body > .markdown').length === 0`;
  the answer renders **12.5px / 21.5px**, not 13/1.64. `chat.css:218` —
  `.narration { font-size: var(--fs-small); }` — is overridden by `chat.css:495`
  `font-size: var(--fs-message);` on the child, so narration prose measures
  **12.5px**, identical to the answer it is supposed to sit behind; only the colour
  differs (5.61:1 vs 9.94:1 pre-fix). `chat.css:199` — `font-size: 12.8px;` on the
  user's card — is off-scale (a 0.3px delta from `--fs-message`).
  **Fix:** `.msg-assistant .markdown { font-size: var(--fs-body); line-height:
  var(--lh-body); }` (drop the `>`), `.narration .markdown { font-size:
  var(--fs-tiny); line-height: 1.6; }`, user card `font-size: var(--fs-message);`.
  Then a turn has two sizes that mean two things instead of four declarations that
  render as one.

- **M2 — inside a card the elevation ladder runs backwards.**
  Measured (current build, dark): card `#1e1c1b` L 11% → code block `#181716`
  L 9% (1.05:1 against its parent) → diff body `#151312` L 8% (1.09:1) → while
  the *head/footer* of both jumps to `#2b2926` L 16% (`chat.css:660`
  `background: var(--surface-inset);`, `chat.css:1906` same). So a diff reads:
  L 16 header, L 8 body, L 16 footer, inside an L 11 card. The card's own border
  is 1.54:1 against its fill and 1.75:1 against the canvas, and the only thing
  separating a card from the page is that hairline.
  **Fix (§3.1, §3.5):** nested blocks step *up*, never down — code/diff bodies on
  `--theme-raised` (L 14%) or `--theme-surface` (L 11%) with the card on
  `--theme-surface` and chrome headers on `--theme-raised`; one border token for
  every card (`--line-strong`), and depth from `--shadow-menu` only, as §5.5
  requires. Delete the `--line-diff*` tones.

- **M3 — in light mode the whole ladder collapses to one flat step.**
  Measured (Daybreak): card fill vs canvas **1.02:1** (L 96 vs L 95 = 1 L point,
  §3.1 requires ≥ 3), composer fill 1.02:1, inset 1.04:1, diff 1.08:1, code
  1.04:1. The only separation between the conversation and its cards is a 1px
  border at 1.49:1. The dock, which in dark is a visibly lifted panel, is
  indistinguishable from the page in light.
  **Fix:** re-author the light ladder to the same ≥ 3-L-point rule (§3.1):
  canvas L 95 → surface L 90 → raised L 84 → hover L 78, or darken the canvas so
  a white card is 3+ points above it. The ladder, not the border, carries depth.

- **M4 — thirteen line variables where §3.5 allows three.**
  `chat.css` + `composer.css` use `var(--line)` ×16, `--line-card-strong` ×14,
  `--line-strong` ×11, `--line-hair` ×5, `--line-divider` ×5, `--line-soft` ×3,
  `--line-inset` ×3, `--line-preview` ×2, `--line-menu` ×2, `--line-symbol`,
  `--line-diff`, `--line-diff-head`, `--line-command` ×1. They resolve to eight
  distinct colours between `#2e2b29` and `#4b4237` — a 1.3:1 spread — so the
  transcript is drawing what looks like one hairline in eight dialects
  (`chat.css:659` `border-bottom: 1px solid var(--line-inset);`,
  `chat.css:909` `… var(--line-divider);`, `chat.css:1801` `… var(--line-diff-head);`
  all on adjacent component heads).
  **Fix:** §3.5's three — `--line-soft` for internal dividers, `--line` for card
  borders, `--line-strong` for hover/focus emphasis — and delete the other ten
  aliases from `tokens.css`.

- **M5 — radii: 62 declarations, 42 literals of 12 distinct values.**
  `chat.css` 23 literals (4, 5, 6, 7, 10, 17, 50%, 999px, 0), `composer.css` 19
  (4, 5, 6, 7, 8, 9, 999px, inherit, 0). §3.4 has six values. The card family is
  consistent at 10px (`chat.css:892, 1790, 1610, 1301`), and then everything
  *inside* it disagrees: the tool symbol `chat.css:919` 7px, the status chip
  `chat.css:302` 5px, the model pill `chat.css:370` 4px, inline code `chat.css:620`
  5px, the review button `chat.css:1928` 6px — four radii nested inside a 10px
  card. The dock is `composer.css:34` `--radius-composer: 15px;` (the largest
  radius in the product; §3.4's top step is 14px) and the send square
  `composer.css:565` 8px.
  **Fix:** `--radius` (10px) for cards and the composer, `--radius-sm` (6px) for
  chips/buttons/inline code, `--radius-xs` (4px) for the model pill, `--radius-lg`
  (14px) for the empty-state symbol — and delete `--radius-composer`. 42 literals
  become 0.

- **M6 — 199 spacing declarations, zero tokens.**
  `node scripts/design-audit.mjs` reports 180 spacing violations in `chat.css` and
  19 in `composer.css`; `grep -c "var(--space-"` is **0** in both. Examples that
  visibly set the rhythm: `chat.css:109` `margin: 0 0 33px;` (message gap),
  `chat.css:167-168` `height: 22px; margin: 0 0 9px;` (message header),
  `chat.css:978` `padding: 13px 16px;` (tool body), `chat.css:1106`
  `min-height: 33px;` (tool footer), `chat.css:1315-1316` `min-height: 31px;
  padding: 7px 10px;` (group head), `composer.css:111` `padding: 12px 13px 3px;`.
  **Fix (§3.2):** snap to the 10-step scale — 33→`--space-10` (32px) or
  `--space-9` (24px), 22→`--space-8` (20), 9→`--space-5` (10), 13→`--space-6`
  (12), 31→`--space-10` (32), `12px 13px 3px`→`var(--space-6) var(--space-6)
  var(--space-2)`. Maximum movement 2px, so nothing reflows.

- **M7 — the composer's only focus affordance is a 1px border; in light it is
  2.27:1.**
  `composer.css:123-127` — `.composer-input:focus, .composer-input:focus-visible {
  outline: none; box-shadow: none; }` — removes the global ring (`base.css:132`)
  and leaves `composer.css:52-54` `.composer:focus-within { border-color:
  var(--focus-input); }`. Measured with transitions disabled: focused border
  `#89755a` — **3.49:1** against the dark composer fill (passes §4's 3.0 floor by
  0.49) and **2.27:1** in Daybreak (`#ca9d81` on `#f7f7f7`) — a fail. `--focus-ring`
  is not used anywhere in either file.
  **Fix (§5.6, §4):** restore a ring on the input itself —
  `outline: 2px solid var(--focus-ring); outline-offset: 2px;` — with
  `--focus-ring` defined per theme at ≥ 3:1 against the composer fill, and keep
  the border colour change as a secondary cue. Do not use `outline: none` as a
  focus reset; the component sets `box-shadow` itself.

- **M8 — readable metadata is painted with `--faint`, which §4 restricts to
  decoration.**
  `chat.css:178` `.msg-time { color: var(--faint); }` (10px, one per row),
  `chat.css:72` `.date-marker` (10px), `chat.css:278-280`
  `.process-fold-meta { color: var(--faint); font-size: var(--fs-micro); }`,
  `chat.css:856-859` `.thinking-hint` (9px), `composer.css:804`
  `.compose-foot { color: var(--faint); font-size: 9.5px; }`. Dark now measures
  5.37:1 and light 3.62:1 — the dark pass is luck, the rule is still broken, and
  the light theme demonstrates why the rule exists.
  **Fix:** `--muted` + `--fs-tiny` for timestamps, date marker, fold meta and the
  footnote; reserve `--faint` for the disclosure chevrons and the list markers
  (`chat.css:571` `li::marker`) where §4 allows it.

- **M9 — hover states: two of the three transcript hovers are invisible, and one
  of them uses the wrong token.**
  `chat.css:1178-1181` `.tool-row-head:hover { border-color:
  var(--line-card-strong); background: var(--surface-card); }` — a row on the
  canvas changes by **1.14:1** (card vs canvas), i.e. not at all.
  `chat.css:1326` `.tool-group-head:hover { background:
  var(--surface-command-hover); }` — `#2a2725` on the card is **1.14:1**.
  `chat.css:1429-1431` `.command-summary:hover { background: var(--surface-hover); }`
  is the only one that follows §5.4.
  **Fix:** all three take `--surface-hover` (L 20%, 1.54:1 against the canvas,
  1.35:1 against a card) and a `--line`→`--line-strong` border step; no hand-mixed
  hover shades (§5.4). The transcript has no selected state to audit; if rows
  become selectable, the accent tint (`--accent-bg` + `--row-accent-ink`) is the
  documented pair.

- **M10 — code block, inline code, tool output and diff rows are the same 11px and
  nothing else.**
  `chat.css:699-700` `.code-block-pre { font-size: 11px; line-height: 1.75; }`,
  `chat.css:933` `.tool-command` and `chat.css:982-983` `.tool-body`
  `font-size: 11px; line-height: 1.8;`,
  `chat.css:1855-1856` `.diff-line { font-size: 11px; line-height: 1.85; }` —
  three line heights for one code surface; three gutters
  (`13px 16px`, `13px 15px`, `0 13px`); three borders (`--line-inset`,
  `--line-diff`, none) and two fills (`--code-bg` vs tinted rows). Inline code
  (`chat.css:617-624`) adds a 5px radius, a `--line` border and accent ink that no
  fenced block shares. Measured in the rendered diff: body 20.3px per line vs
  code 19.25px vs tool output 19.8px.
  **Fix:** one code surface (`--theme-raised` per §3.1), one line token
  (`--line-soft`), one type step (`--fs-tiny`), one line-height token
  (`--lh-code: 1.75`), one gutter (`var(--space-6) var(--space-7)`), and status
  colour used as *ink only* on diff rows.

## Minor

- **m1 — off-scale font weights.** `chat.css:271` `font-weight: 560;`,
  `chat.css:551` `620`, `chat.css:75/1620/2006` `650`, `composer.css:289` `650`,
  `composer.css:394` `620`, `composer.css:865` `540`, `chat.css:612/775` `560` —
  ten declarations outside §3.3's `{400, 500, 550, 600, 660, 700, 720, 800}`. Snap
  each to the nearest allowed value (560→550, 620→600 or 660, 650→660, 540→550).

- **m2 — three one-off radii outside the card family.** `composer.css:34` 15px
  (composer), `composer.css:565` 8px (send), `chat.css:2126` 17px (empty-state
  symbol). Under §3.4: `--radius` (10), `--radius-md` (8), `--radius-lg` (14).

- **m3 — the message header packs four heights into 22px.**
  `chat.css:167` `height: 22px;` holds the 29px avatar, a 17px `.tag`
  (`chat.css:299`), a 17px `.model-pill` and an 18px `.tps-badge`
  (`chat.css:389`). Align on `--space-8` (20px) or let the row size itself.

- **m4 — one dashed border in the transcript.** `chat.css:253` `.process-fold {
  border: 1px dashed var(--line-card-strong); }` — nothing else in the surface is
  dashed; §3.5's three line tokens do not include a dash style. Use a solid
  `--line` and the disclosure chevron already in the button.

- **m5 — the rewind control is 10px and invisible until hover.**
  `chat.css:2519-2534` `.rewind-button { font-size: var(--fs-micro); opacity: 0;
  }` with `chat.css:2537-2539` `.msg:hover .rewind-button { opacity: 1; }`. It has
  a `@media (hover: none)` fallback (`chat.css:2554-2558`), but it is still a
  10px `--text-subtle` label (`chat.css:2528`) — make it `--fs-tiny` + `--muted`,
  and show it on row focus as well as hover (it already does via `:focus-visible`).

- **m6 — the type scale's new steps are unused in this surface.**
  `--fs-tiny`, `--fs-label`, `--fs-lead`, `--fs-title`, `--fs-heading`,
  `--fs-display`, `--fs-hero` and `--fs-body` appear **0 times** in `chat.css` and
  `composer.css` (`--fs-micro` 32×, `--fs-small` 8×, `--fs-message` 2×; the
  composer has exactly **one** token use out of its 36 `font-size`
  declarations). Migrating M1/B4 to the scale will consume them; until then the
  tokens exist but the surface cannot be said to use them — the UI body size
  (`--fs-body`) is not used once.

- **m7 — `composer.css` is nearly untouched by the token layer.** 36 `font-size`
  declarations → 1 token; 22 radii → 3 tokens; 19 spacing declarations → 0 tokens.
  It should be the second file on the migration list after `chat.css`.

## What I would fix first

1. **B1 — define `--surface-2`/`--surface-3`** (or use `--surface-raised` /
   `--surface-hover`). Seven invisible states, one-line fix, no reflow.
2. **B3 — re-derive the diff row fills from `--success-bg`/`--danger-bg`.** 2.88:1
   and 2.81:1 on the one surface a user must read before approving a write.
3. **B2 — light-mode text: `--faint` out of metadata, `--text-secondary` into tool
   output.** 73 of 168 nodes fail; the tool output (4.27:1) and line numbers
   (3.15:1) are the product.
4. **M1 — repair the two dead reading-size rules.** The documented 13px answer
   size has never rendered; narration and answers are the same size today.
5. **B4 — delete 9px and 9.5px** in favour of `--fs-micro`/`--fs-tiny`. Twenty
   declarations, and it is the difference between §3.3 being a scale and being a
   suggestion.