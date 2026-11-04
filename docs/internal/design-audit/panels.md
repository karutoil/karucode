# Panel and tool-surface audit

**Domain:** `inspector.css`, `terminal.css`, `git.css`, `todo.css`, `goals.css`,
`session-control.css` and the components they paint (Inspector, FileTree,
FileViewer, FilePreview, WorkspaceTab, SessionInfoTab, AgentsTab, ControlPlane,
TerminalTab, TerminalSurface, TodoPanel, GoalsPanel, PluginPanelView,
`components/git/*`).

**Snapshot.** Live measurements come from the build served on
`http://127.0.0.1:8944` (Ember dark at 1440×960 and 1023×900, Daybreak light at
1440×960), read with `getComputedStyle` and in-page WCAG maths. Static counts
come from `node web/scripts/design-audit.mjs`. `tokens.css` and `theme/*` were
being migrated *during* this audit (`git status`: `M web/src/styles/tokens.css`,
`M web/src/theme/*`, dist rebuilt at 11:53); the six component stylesheets in
this domain are untouched. Where a token value moved mid-audit, both numbers are
given and the pre-migration one is marked *(old build)*.

---

## Verdict

The panels are well-structured and now correctly coloured *at the shell level*,
but they have not joined the design-system migration: 175 of the app's 414
hand-written `font-size`s (42%), 367 of 1092 spacing literals, 67 of 172 radii
and every one of the escaping palettes live in these six files, and four of the
six consume **zero** `--fs-*` tokens. The concurrent token migration fixed the
thing DESIGN-SYSTEM §1.1 complained about — the inset card that measured
`#1f2630` (h215 s22% l15%) on a `#171819` panel (h210 s4% l9%) now derives
`#2b2926` (h36 s6% l16%) on `#181716` (h30 s4%), and panel text contrast went
from 11 of 61 nodes below 4.5:1 to 0 of 64 — but it did so entirely in
`tokens.css`/`roles.ts`; the component layer still paints 18 card fills from
role literals instead of the §3.1 ladder, which is why the panels have no
readable depth (measured 1.00–1.23:1 between every adjacent surface, four levels
deep), why a 1241px diff line is displayed in a 238px box with no scroll, and
why three separate palettes (todo's dead status fallbacks, session-control's twelve
raw colours the audit cannot see, and a terminal whose ANSI blue renders amber)
still contradict the theme. Verdict: **the token layer is ready; the panel layer
is not, and the three highest-impact defects are structural, not chromatic.**

### What the migration changed (so the reader is not misled)

| Surface | Old build | Current build | HSL (current) |
| --- | --- | --- | --- |
| inspector panel | `#171819` | `#181716` | h30 s4% l9% |
| inset card | `#1f2630` | `#2b2926` | h36 s6% l16% |
| agent card | `#13181f` | `#1e1c1b` | h20 s5% l11% |
| theme surface | `#1c1d1f` | `#23211f` | h30 s6% l13% |
| theme raised | `#222326` | `#2c2a28` | h30 s5% l16% |

Session-tab text nodes below 4.5:1: **11 of 61 (old build) → 0 of 64 (current)**.
The §1.1 saturation clash is closed. The §1.2, §1.4 and §1.5 defects are not.

---

## Blockers

### B1 — The elevation ladder is still invisible inside the panels (1.00–1.23:1)

§1.2 says adjacent steps must be visible; §3.1 says each step is ≥3 L points
(the published table itself steps by 2, 1 and 0 between rail, sidebar and
inspector, so only its card steps survive that rule as authored).
Measured on the current build, every card-to-panel step in this domain is below
1.25:1, and the ladder is not monotonic:

| Pair | Ratio |
| --- | --- |
| `.agent-card` `#1e1c1b` on inspector `#181716` | **1.05** |
| `.agent-event` `#1e1c1b` inside `.agent-card` `#1e1c1b` | **1.00** |
| `.agent-bubble` `#2b2926` inside that card | 1.17 |
| `.metric` `#2b2926` on the panel | 1.23 |
| `--surface-inset` `#2b2926` vs `--theme-raised` `#2c2a28` | **1.01** |
| `--surface-inset` `#2b2926` vs `--theme-surface` `#23211f` | 1.11 |

- `web/src/styles/inspector.css:1065` — `background: var(--surface-command);`
  (`.agent-card`). The card is 1.05:1 from the panel it sits on; only the 1px
  `--line-card` border says "card".
- `web/src/styles/inspector.css:1402` — `background: var(--surface-command);`
  (`.agent-event`) inside that same card: **identical fill, 1.00:1**. This is a
  card-in-card with no elevation at all.
- `web/src/styles/inspector.css:1357` — `background: var(--surface-inset);`
  (`.agent-bubble`) is *brighter* than the card that contains it, so the nesting
  reads as a hole in a hole.
- `web/src/styles/inspector.css:705` — `background: var(--surface-inset);`
  (`.metric`, the Session tab's eight tiles) — 1.23:1, the "deepest" step in the
  panel.
- `web/src/styles/tokens.css:89` — `--surface-inset: #2b2926;` is L16, while
  `--theme-surface` (`tokens.css:25`) is L13 and `--theme-raised`
  (`tokens.css:26`) is L16. An **inset** is now brighter than a card and
  indistinguishable from a raised surface (1.01:1), which inverts the word.

**Fix.** Use the §3.1 ladder only, one step per level and always upward:
pane = `--theme-inspector` (L8), card = `--theme-surface` (L11), nested card =
`--theme-raised` (L14), hover/active = `--theme-hover` (L17). Reserve
`--theme-bg` (L5) for genuinely recessed output wells. Retire
`--surface-inset`/`--surface-command`/`--surface-card` from *card* roles in
`inspector.css` (13 inset + 5 command uses) and keep at most one well token,
mapped to `--theme-bg`.

### B2 — The diff pane clips 81% of every long line and the preview clips code silently

- `web/src/styles/chat.css:1871` — `.diff-text { overflow: hidden; text-overflow: ellipsis; white-space: pre; }` (rule at `:1868`)
  inside `.diff-lines` (`chat.css:1844`) which declares `overflow-x: auto` (`:1846`).
  Measured at 342px inspector width: box **238px**, content **1241px**,
  `.diff-lines` `scrollWidth == clientWidth == 291` — the scroll never engages
  because the child hides the overflow. 1003px (81%) of the line is unreachable
  by scroll, wrap, or copy.
- `web/src/styles/chat.css:1876` — `.diff-line.add { background: var(--surface-diff-add); color: var(--success); }` (`:1877–1878`).
  Contrast is fine (`#88baa0` on `#172920` = 6.97:1; delete `#e49e91` on
  `#38150e` = 7.49:1), so the brief's contrast hypothesis is disproved — the
  defect is that the code is not readable *at all*, not that it is low-contrast.
- `web/src/styles/inspector.css:511` — `.preview-code { max-height: 8.6em; overflow: hidden; ... font-size: 10px; line-height: 2; }` (`overflow` at `:515`, `font-size` at `:518`).
  Measured: 291px box, **1040px** content, no ellipsis (`white-space: pre`), no
  scroll. The quick preview shows 8 lines of 10px code and amputates the rest.
- `web/src/styles/git.css:669` (`.gitm-detail-head`, `padding` overridden at
  `git.css:901`) — in a 291px header, `.gitm-detail-actions` takes **175.5px**
  (three buttons) and `.gitm-detail-titles` is squeezed to **81.5px**: at 12px
  mono that is ~11 characters for the filename, and the sub-line wraps to 52px
  (four lines). A diff header in the inspector cannot show
  `crates/kc-server/src/routes/terminals.rs`.

**Fix.** Let the scroller own the scroll: remove `overflow: hidden` from
`.diff-text`, give each `.diff-line` `min-width: max-content`, keep
`.diff-lines` `overflow-x: auto` with a sticky gutter. For the header, give
`.gitm-detail-titles` `flex: 1 1 0` and `.gitm-detail-actions`
`flex: 0 1 auto; flex-wrap: wrap`; budget the header padding with `--space-3`
/ `--space-4` instead of `10px 11px 9px`.

### B3 — 175 hand-written font sizes; the panel has not adopted `--fs-*` at all

`node scripts/design-audit.mjs` (current source):

- **175** `font-size` declarations outside `--fs-*` in these six files — **42%
  of the app's 414**. `inspector.css` alone: **93**, in **11 distinct values**:
  `10px ×28`, `11px ×17`, `10.5px ×17`, `9.5px ×15`, `9px ×7`, `12px ×3`,
  `11.5px ×2`, `13px ×1`, `15px ×1`, `21px ×1`, `23px ×1`.
  **67 of the 93 (72%) are below the §3.3 11px floor.**
- `var(--fs-*)` usage in these files: `git.css` 27, `terminal.css` 8,
  **`inspector.css` 0, `todo.css` 0, `goals.css` 0, `session-control.css` 0** —
  3,735 lines with no type token.
- Worst offenders, all persistent metadata a user must read:
  `inspector.css:1044` `font-size: 9px` (`.agent-filter-count`),
  `:1151` `9px` (`.agent-live`), `:1217` `9px` (`.agent-badge`),
  `:1371` `9px` (`.agent-bubble-role`), `:1439` `9px` (`.agent-event-status`),
  `:2078` `9px` (`.si-agent-meta dt`),
  `:438` `9.5px` (`.tree-size`), `:868` `9.5px` (`.change-status-label`),
  `:1019` `9.5px` (`.agent-filter`), `:1290` `9.5px` (`.agent-detail-kicker`),
  `:2151` `9.5px` (`.si-run-meta`).
  Off-scale display sizes: `inspector.css:585` `23px` (`.panel-title`),
  `:711` `21px` (`.metric-num`), `:1861` `15px` (`.si-window-percent`).
- Live Session tab, 64 text nodes: **25 at 10px, 21 at 10.5px, 3 at 9.5px** =
  49 (77%) in the 9.5–10.5px band; only 9 are 15px or larger.
- `web/src/styles/todo.css:139` `font-size: 9px` (`.todo-status`),
  `goals.css:394` `10px`, `session-control.css:344` `9.5px` (`.cp-eyebrow`),
  `:577` `9.5px` (`.cp-table-head`) — same defect, other panels.
- Token drift worth fixing before migrating: the runtime `--fs-tiny` computes
  to **12px** (`theme/tokens.ts:251` derives it from `density.body - 1`) while
  `tokens.css:264` publishes `--fs-tiny: 11px`. A migration to `--fs-tiny` would
  give 12px, not the §3.3 floor.

**Fix.** Map every literal to §3.3: `9px`/`9.5px`/`10.5px` → `--fs-tiny` (11px)
or `--fs-small` (11.5px); `10px` is legal only as `.eyebrow`
(`--fs-kicker` 10px / `--weight-kicker` 720 / `--tracking-kicker` 0.125em);
`12px` → `--fs-label`; `15px` → `--fs-lead`; `21px`/`23px` → `--fs-display`
(20px). Reconcile `--fs-tiny` in `theme/tokens.ts` first.

### B4 — Three palettes still escape the token layer (§1.5, §5.1, §5.3)

**(a) `todo.css` — the second status palette the design doc names.**
`web/src/styles/todo.css:184` `border-color: color-mix(in srgb, var(--warn, #d09a3c) 45%, transparent);`
`:185` `background: ... var(--warn, #d09a3c) 12% ...`,
`:186` `color: var(--warn, #d09a3c);`,
`:190–192` the same for `var(--ok, #3f9d6a)`, plus `:257`, `:258`, `:262`,
`:263`, `:271`, `:304`, `:306` — **13 declarations carrying the two literal hexes**
`#d09a3c`/`#3f9d6a`, all in one file. Both tokens exist (`tokens.css:49` `--theme-warn`,
and `--ok: var(--success)` at `tokens.css:240`), so the fallbacks are dead:
measured `.todo-tone-warn` colour = `rgb(241,198,145)` and `.todo-tone-done` =
`rgb(136,186,160)`, i.e. the theme values. But the fallbacks encode a *different*
palette that would appear if the alias were ever removed: `#d09a3c`
(h38 s61% l53%) vs `--warn` `#f1c691` (h33 s77% l76%) — **ΔL 23**; `#3f9d6a`
(h147 s43% l43%) vs `--success` `#88baa0` (h149 s27% l63%) — **ΔL 20, ΔS +16**.
This is the §1.5 defect verbatim: a component inventing its own reds and greens.

**(b) `session-control.css` — a third palette the audit tool cannot see.**
`web/src/styles/session-control.css:223–225`:
`:223` `color: #b8d6c1; border-color: #3f584c; background: #26332c;`
`:224` `color: #b9cde4; border-color: #3d5064; background: #26313b;`
`:225` `color: #dcc6e4; border-color: #54435f; background: #302a36;`
plus `:265–267` `box-shadow: ... rgb(136 186 160 / 38%)` in `@keyframes cp-pulse`
— **12 raw colour declarations** (nine hexes plus three `rgb()`s). They are
three hue families (h138 s27%, h212 s44%, h284 s36%) in a shell whose hue is now
30, on fills at s13–22%. The audit reports **zero** colour
violations for this file because `scripts/design-audit.mjs:88` only matches a
declaration that starts a line, and these are single-line rules — so the tool's
headline "9 colour + 6 borderColour" is an undercount: these six files contain
**25 raw colour declarations**, of which the tool reports 13.

**(c) The terminal's ANSI palette is not the app's palette.**
`web/src/components/TerminalSurface.tsx:97–115` builds xterm's theme by reading
CSS variables, but three do not exist at `:root`:
`--surface-sunken` (`:97`, empty at runtime → falls back to `--bg`),
`--warning` (`:108`, empty → `#e0af68`) and, in CSS,
`terminal.css:115` `background: var(--warning, var(--accent));` and
`terminal.css:260` `color: var(--warning, var(--accent));`.
Measured live by printing ANSI 0–15 into the terminal:

| Slot | Value | Contrast on terminal `#141312` |
| --- | --- | --- |
| 0 black | `#414868` (h229 s23%) | **2.08** |
| 1 red | `#e49e91` (token) | 8.48 |
| 2 green | `#88baa0` (token) | 8.46 |
| 3 yellow | `#e0af68` (fallback, not `--warn`) | 9.28 |
| 4 blue | `#dfb680` — **the accent: blue renders amber** | 9.84 |
| 5 magenta | `#bb9af7` (h261 s85%) | 8.02 |
| 6 cyan | `#7dcfff` (h202 s100%) | 10.82 |
| 7 white | `#f2f0ee` (token) | 16.33 |
| 8–15 | xterm/Tango defaults (`#555753`, `#ef2929`, `#8ae234`, `#fce94f`, `#729fcf`, `#ad7fa8`, `#34e2e2`, `#eeeeec`) | 8 = **2.54**, 9 = 4.44 (10–15: 11.50 / 14.94 / 6.69 / 5.64 / 11.60 / 15.97) |

So only slots **1, 2 and 7** are token-derived *and* semantically right; slot 4
is token-derived but wrong (blue→accent); the other **12 of 16** are literals,
dead fallbacks or xterm defaults. `terminal.css:333` then sets `.xterm-viewport { background-color:
transparent !important; }`, discarding xterm's own background, so the canvas is
the *rail* colour (`#141312`) rather than `--theme-bg` (`#0f0e0d`) — a 1.04:1
seam, and a terminal chrome (`#23211f` strip) that has no relationship to the
program colours inside it.

**Fix.** (a) `var(--warn)` / `var(--success)` with tints built by
`color-mix(in srgb, var(--success) 12%, transparent)`; delete `--ok` and the
two literal hexes. (b) Use `--accent` for a single selected project tone and
`--muted` for the rest (§2: one hue family; §5.3: no hue the palette lacks);
delete the three tone hexes and the pulse `rgb()`. (c) Read `--theme-bg` (not
`--surface-sunken`), `--warn` (not `--warning`), map ANSI blue to `--info` (the
one blue the palette has), black to `--muted` (clears 3:1; never `--theme-bg`,
which is 1.04:1 on the canvas),
and derive 8–15 as tints of 0–7; remove the fallback literals so a missing token
fails loudly instead of silently importing Tokyo Night.

---

## Major

- **M1 — Progress affordances are invisible.** `goals.css:178` `.goals-bar { height: 4px; background: var(--surface-inset); }` sits on `--theme-raised`: measured 270×4px, track-to-card **1.01:1** — a 40% bar reads as a floating amber stub with no container, and 0% is nothing at all. `inspector.css:1227` `.agent-progress { height: 3px; background: color-mix(in srgb, var(--muted) 22%, transparent); }` (`height` at `:1223`) → **1.50:1** against `.agent-card`. `goals.css:268` `.goals-mark-empty { width: 9px; height: 9px; border: 1px solid var(--line-inset); }` → a 9px unchecked criterion whose only edge is **1.15:1** (`--line-inset` on `--theme-raised`). `GoalsPanel.tsx:458` provides `role="progressbar"` and `aria-valuenow` but no visible percentage; `aria-label` says "N criteria met" while the value is a percentage. Todo has no progress affordance at all, only count chips.
  **Fix:** track = `--theme-bg` (one step *below* the card, ≥1.5:1 at these L values) or `--theme-surface` with a `--line` border; 6px minimum height; a visible `--fs-tiny` percentage from `--muted`; the empty criterion mark ≥14px with a `--line-strong` border.
- **M2 — 67 hand-written radii, including computed ones.** `terminal.css:153` `border-radius: calc(var(--radius) - 7px)` (= 3px), `:138` `calc(var(--radius) - 8px)` (= 2px), `:290`, `:381`; `inspector.css:213` `7px`, `:704` `9px`, `:1226`/`:1233` `2px`, `:152` `3px 3px 0 0`; `git.css:499` `4px`, `:716` `0`; `goals.css:59` `6px`, `:78` `10px`, `:273` `3px`, `:355` `7px`; `session-control.css:50` `11px`, `:84` `9px`, `:501`/`:522`/`:551` `11px`. §3.4 publishes six values; this domain authors **2, 3, 5, 6, 7, 9, 10, 11, 0 5px 5px 0**.
  **Fix:** `4→--radius-xs`, `6→--radius-sm`, `8→--radius-md`, `10→--radius`, `11/14→--radius-lg`, pills→`--radius-pill`; every `calc(var(--radius) - Npx)` becomes `--radius-xs`. A 2px badge radius is a defect, not a style.
- **M3 — 367 spacing literals (34% of the app's 1092) in six files.** Worst: `git.css:242` `padding: 13px 14px 11px`, `git.css:46` `padding: 15px 20px 12px`, `git.css:902` `padding: 10px 11px 9px`, `inspector.css:169` `padding: 16px 17px`, `goals.css:409` `margin: 6px 0`, and `gap: 1px` used as rhythm (`git.css:429`, `:444`, `:535`, `:954`, `:1008`). Per file: inspector 159, git 76, session-control 56, goals 27, terminal 26, todo 23.
  **Fix:** snap to `--space-1..10`; maximum movement is 2px, so no layout reflows; `1px` becomes `--hairline`.
- **M4 — Nine off-palette font weights.** §3.3 allows `{400,500,550,600,660,700,720,800}`; measured in these files: `620` (`git.css:75`), `640` (`git.css:370`), `650` (`inspector.css:1126`, `:1862`, `:1926`, `todo.css:140`, `session-control.css:380`, `:688`), `670` (`inspector.css:712`), `680` (`session-control.css:335`), `690` (`inspector.css:586`), `710` (3×), `740` (`inspector.css:42`), `750` (`session-control.css:55`). 16 declarations use a value outside the set (650 ×6, 710 ×3); `740`/`750` are one-offs.
  **Fix:** collapse to 600/660/700/720.
- **M5 — The file-tree icon set is three indistinguishable tans plus one cool hue.** Live: `.kind-folder` `#aea18f` (h35 s16% l62%), `.kind-file` `#b2aaa0` (h33 s10% l66%), `.kind-rust` `#b7ab9b` (h34 s16% l66%) — ΔL ≤ 4, Δh ≤ 2, so the three "different" icons read as the same beige; `.kind-md` `#a9b4c0` (h211 s15% l71%) is the only cool hue in a warm panel, and `.kind-ts` is `--info` (`#92b3d8`, h212 s47%). That is a fourth hue family by §2's definition even after the retint. Contrast is fine (7.07 / 8.51 / 5.43 on the panel).
  **Fix:** one hue family — folder `--accent`, file `--muted`, open/selected `--text-secondary`; delete `--icon-folder`/`--icon-rust`/`--icon-md`/`--icon-path` or add a documented two-tone icon pair to §3.3.
- **M6 — Status is communicated by a 6px dot with the label deleted.** `session-control.css:258` `.cp-state.good, .cp-state.idle { gap: 0; font-size: 0; }` — measured `.cp-state` computed font-size **0px**, so "Finished" exists only as a 6px `--muted` dot; `:783–784` does the same at ≤520 to `.cp-state` and `.cp-secondary`, replacing the text with `::after { content: '↗' }`. A colour-only status cue also fails WCAG 1.4.1 and §2 ("status colours … appear only on status" presumes a status you can read).
  **Fix:** keep the label at `--fs-tiny` and let the dot carry `--success`/`--warn`/`--danger`; at ≤520 keep a one-word label, not a glyph.
- **M7 — The inspector is 1px wider than its own content box at every width.** `layout.css:1548` `.inspector-inner { width: var(--inspector-w); }` (rule at `:1545`) inside `.inspector { flex: 0 0 var(--inspector-w); border-left: 1px solid … }`: measured `.inspector` clientWidth **341** / `.inspector-inner` width **342** at 1440 (inner right edge **1441** vs viewport **1440**), and at 1023 the open sheet's inner right edge is **1024**. `overflow: hidden` on `.inspector` hides the symptom — the last 1px of the tab strip, scrollbar and panel is clipped.
  **Fix:** `width: 100%` on `.inspector-inner` (border-box already applies).
- **M8 — Tree indentation is an inline pixel formula with no cap.** `FileTree.tsx:189` `style={{ paddingLeft: `${node.depth * 18 + 8}px` }}` — measured 8px at depth 0 and 26px at depth 1 in a 293px row. `MAX_TREE_DEPTH = 32` (`FileTree.tsx:24`) means a pathological listing indents **584px** inside a 293px row: the name is pushed off-screen with no horizontal scroll. Nothing here is a `--space-*` token.
  **Fix:** `padding-left: calc(var(--space-4) + var(--indent-step) * min(var(--depth), 8))` with `--indent-step: 14px` declared once; clamp the depth.
- **M9 — Three tokens are read but never defined.** Verified empty at `:root` on the current build: `--surface-sunken` (`TerminalSurface.tsx:97`), `--warning` (`TerminalSurface.tsx:108`, `terminal.css:115`, `terminal.css:260`), `--mono` (`session-control.css:632` `font-family: var(--mono, ui-monospace, monospace)`). Their fallbacks are hardcoded colours/faces, so a theme change silently stops applying. `--ink-secondary` (`TerminalSurface.tsx:105`) *is* defined — but on `.app` (`layout.css:17`), not `:root`, so the lookup misses and ANSI black is the Tokyo Night literal `#414868`.
  **Fix:** `--theme-bg`, `--warn`, `--font-mono`, and read `--ink-secondary` from `document.querySelector('.app')` or move the definition to `:root`.

---

## Minor

- **m1 — The only remaining AA failure in the panel is the diff gutter.**
  `chat.css:1863` `.diff-line i { color: var(--faint); }` on the added-row fill
  `#172920` = **4.25:1** at 11px (line numbers); on the context row 5.06, on the
  panel 4.98. §4 floors `--muted` at 4.5. Fix: `--muted` for the gutter, or
  `--faint` only on the non-tinted rows.
- **m2 — Code and preview prose are set at 10px.** `inspector.css:518`
  `.preview-code` (mono, `line-height: 2`) and `:531` `.preview-placeholder`
  (prose) use `font-size: 10px`; `:1723` `.plugin-panel-json` uses 10.5px.
  §3.3's 10px `--fs-micro` is for uppercase kickers only, and the prose floor
  is 11px. Fix: `--fs-tiny` (11px) for code blocks and preview prose.
- **m3 — Disabled commit button text is 3.98:1.** `git.css:324`
  `.gitm-commit-btn:disabled { color: var(--faint); background: var(--surface-raised); }` (`color` at `:327`)
  (`#8d867e` on `#2c2a28`). Disabled controls are exempt from WCAG, but §4 does
  not scope its floors; use `--muted`.
- **m4 — Literal motion and hand-authored depth.** `goals.css:189`
  `transition: width 160ms ease;` (not `--transition`); `inspector.css:999`
  `box-shadow: 0 0 0 1px var(--line-card)`; `git.css:463`
  `box-shadow: inset 2px 0 0 var(--accent)`; `session-control.css:265–267` the
  pulse glow. §5.5 allows two shadow tokens and no third; §5.7 requires
  `--transition`.
- **m5 — Dense data labels at 9.5px uppercase.** `session-control.css:344`
  `.cp-eyebrow`, `:577` `.cp-table-head`, `:362` `.cp-stats span`. A
  four-column table header at 9.5px / 0.1em is legible only on a perfect panel.
  Fix: `--fs-micro` with `.eyebrow`'s 10/720.
- **m6 — Hit targets below 24px.** `todo.css:97` `.todo-item-toggle` 14×14px
  (the only way to expand a task), `goals.css:51` `.goals-refresh` 22×22,
  `inspector.css:272` `.inspector-mini` 24×24, `:1695` `.plugin-panel-refresh`
  24×24. Fix: 24px minimum box with the glyph unchanged.
- **m7 — The tree paints selection from focus.** `FileTree.tsx:183`
  `aria-selected={selected}` where `selected = focusedPath === node.entry.path`
  (`:175`), so merely arrowing through the tree paints every row with
  `--row-selected-bg` (`--accent-bg`) and `--row-selected-text` (`--accent`).
  The accent is reserved for selection (§2); focus is not selection. Fix:
  `aria-selected` from the opened path, `:focus-visible` from `--focus-ring`.
- **m8 — Branch pills and the first row duplicate the tab's chrome.**
  `git.css:176` `.gitm-branch` (a full-width 28px pill on `--surface`) sits
  above `git.css:892` `.gitm-panel-detail` (a card on `--surface`) inside the
  inspector: three nested rounded surfaces in the first 90px of the tab. Fix:
  one card — the branch becomes a `.gitm-group-head`-style row.

---

## The five defects I would fix first

1. **B1 — make depth readable.** Map `inspector.css`'s 18 card fills to
   `--theme-surface` / `--theme-raised` / `--theme-bg` and stop using
   `--surface-inset` as a card. Nothing else in the panel will look intentional
   while a card and its parent are 1.05:1 apart.
2. **B2 — let the diff scroll.** Remove `overflow: hidden` from `.diff-text`;
   81% of every long line is currently unreachable, which makes the Git panel
   unusable for its only job.
3. **B4a — delete the second status palette in `todo.css`** (13 declarations,
   two ΔL-20 fallbacks) before the `--ok` alias is retired and the panel turns a
   different green.
4. **B4b — bring `session-control.css` into the audit.** Twelve raw colours in six
   lines are invisible to `design-audit.mjs`; fix the tool's one-declaration-per-
   line assumption and the palette, in that order.
5. **B3 — collapse the 175 font sizes to `--fs-*`** (and fix `--fs-tiny` to 11px
   in `theme/tokens.ts` first), starting with `inspector.css`'s 93.
