# The application shell — design audit

Files audited: `web/src/styles/layout.css`, `web/src/styles/rail.css`, `web/src/styles/base.css`,
and the components they paint: `ProjectRail.tsx`, `SessionsRail.tsx`, `BootScreens.tsx`,
`EmptyState.tsx`, `AuthScreen.tsx`, `Tabs.tsx`, `app.tsx`. Framed against
[`DESIGN-SYSTEM.md`](../DESIGN-SYSTEM.md) (§ references below are its sections).

**Method.** Static: `node web/scripts/design-audit.mjs --detail` (per-file counts, `file:line`).
Live: Chromium over CDP against `http://127.0.0.1:8941` (the `web/dist` build served at 11:53),
viewport 1440×960, 1920×1080, 1100×960, 1024×960, 1023×960, in Ember (dark) and Daybreak (light).
Contrast and HSL were computed in the page from `getComputedStyle` (effective background composited
through every translucent ancestor; element `opacity` folded into the text colour). Geometry is
`getBoundingClientRect`. Hover states could not be driven — the automation's pointer never produced a
`:hover` match (`document.querySelectorAll(':hover')` was empty after hovering two controls) — so
pointer/focus states were resolved by applying each state rule's own declaration through a stylesheet
and reading the computed value; that is exact for the `var()`-invalid cases below and is labelled as
such. Note: the palette/token layer (`web/src/theme/*`, `tokens.css`) was being rewritten while this
audit ran (mtime 11:53); every number below is from that build and should be re-run after the palette
work lands.

---

## Verdict

The shell's **colour** problem has moved: measured on this build, every shell surface is one warm
family — canvas `#0f0e0d` H30 S7 L5, project rail `#141312` H30 S5 L7, inspector `#181716` H30 S4 L9,
input surface `#23211f` H30 S6 L13 — so §1.1's three-clashing-families diagnosis (neutral 210° greys vs
20–24% blue-slate vs a 34° amber) **no longer reproduces in the shell**, and dark-mode shell text fails
AA on only 3 of 34 nodes. What is left is structural and worse: the shell's **own** declarations defeat
the token layer in four places that matter. Two aliases the shell depends on (`--surface-2`,
`--surface-3`) resolve to nothing, so nine declarations — the selected session row, every hover fill on
the primary action, the search field's focus fill, the context-meter track, the project tiles — paint
transparent (measured `rgba(0,0,0,0)`); the sessions column paints the project rail's exact colour
(ΔL 0, 1.00:1) because it asks for `--panel-2` instead of the `--sidebar` token that exists for it;
fifteen text roles still take `--faint`, which §4 reserves for decoration, which is why **20 of 34**
shell text nodes fail 4.5:1 in Daybreak (worst 2.38:1 at 9px); and two focus rules either delete the
ring or replace it with a cue of 1.12–1.53:1. On top of that the ladder is only 1.00–1.14:1 between
adjacent steps, the header clips its own status pill at 1024px (9.3 of 50.6px visible) and the composer
dock drifts from the transcript by 1.0 / 7.5 / 9.5px depending on width. The scale violations are
concentrated but real: 260 token-rule violations in these three files (36 + 8 + 6 font sizes, 19 + 7 + 1
radii, 122 + 35 + 26 spacings), including 12 declarations at 9–9.5px that §3.3 says are gone.

---

## Blockers

- **B1 — `--surface-2` and `--surface-3` are undefined, so nine declarations paint nothing.**
  `web/src/styles/tokens.css:334` — `--surface-2: var(--raised);` and `:335` — `--surface-3: var(--hover);`.
  Neither `--raised` nor `--hover` is defined anywhere in `web/src/styles` (0 definitions; only
  `--surface-raised` and `--surface-hover` exist). `var()` referencing a missing custom property makes
  the declaration **invalid at computed-value time**, so it does not fall back to the base rule — it
  becomes transparent.
  Measured: computed `--surface-2` = `""`, `--surface-3` = `""`; `.session-row.is-active` computed
  `background-color` = `rgba(0, 0, 0, 0)`; a probe element with `background: var(--surface-2)` (the exact
  declaration from `layout.css:236`) computes `rgba(0, 0, 0, 0)`; same for `var(--surface-3)`.
  Affected, all in my files: `layout.css:469` `.session-row.is-active` (the selected row has **no fill** —
  it renders identical to every other row), `:236` `.new-session:hover` (the primary action *loses* its
  `#23211f` fill on hover), `:294` `.side-search:focus-within`, `:910` `.settings-btn:hover`, `:1155`
  `.head-btn.head-icon:hover` (the header's four icon buttons have no hover fill), `:1323`
  `.workspace-tab .tab-count`, `:600` `.session-ratio` (invisible bar), `:879` `.side-meter-bar`
  (invisible context-meter track — only the filled part shows), `rail.css:145` `.project-rail-avatar`
  (project tiles have **no tile fill**: a probe computes `rgba(0, 0, 0, 0)`; only the active tile gets one
  from `.is-active` at `rail.css:216`).
  Fix: `tokens.css` — `--surface-2: var(--theme-raised); --surface-3: var(--theme-hover);`, i.e. point
  them at the §3.1 rungs. (Or drop the aliases and rewrite the nine call sites to the §3.1 public names
  `--surface-raised` / `--surface-hover`.) Note §3.1's raised is L14 and hover L17 against the current
  surface L13, so this also repairs a ladder step.

- **B2 — The sessions sidebar and the project rail are painted the same colour.**
  `web/src/styles/layout.css:40` — `background: var(--panel-2);` and `tokens.css:333` —
  `--panel-2: var(--rail);` (also `rail.css:29`). The sessions column asks for the *chrome* fill rather
  than the sidebar rung, so the two adjacent columns are one field separated only by a 1px border.
  Measured: project rail `#141312` and sessions rail `#141312` — H30 S5 L7 both; **ΔL 0, contrast
  1.00:1**. Light mode: both `#fafaf7`. `--sidebar` is `#1a1918` (H30 S5 L10) in Ember and `#f3eee7`
  in Daybreak, and is consumed by `inspector.css:1564`, `overlays.css:312`, `settings.css:1237`,
  `theme-studio.css:32` and `:851` — but never by the shell it was named for.
  Fix: `layout.css:40` → `background: var(--sidebar);` giving ΔL 3 and 1.06:1 — §3.1's "session list"
  row. If `--panel-2` stays, point it at `--theme-sidebar`, not `--theme-rail`.

- **B3 — In light mode 20 of 34 shell text nodes are below 4.5:1, because the shell paints readable
  metadata with `--faint`.**
  §4: "`--faint` — decorative only … Never for text a user must read." These three files use
  `color: var(--faint)` **15 times** for text (`layout.css:184, 259, 285, 301, 316, 404, 561, 587, 648,
  819, 868, 1038, 1078, 1308, 1343`, plus `rail.css:512`, `base.css:268, 323`).
  Measured in Daybreak (light), effective contrast including element opacity: `.session-turns`
  (`layout.css:586` 9px × `:588` `opacity: 0.7`) **2.38:1**; `.session-group-label span` (`:416`
  `opacity: 0.8`) **2.76:1**; `.filter-chip-count` (`:360` 9px × `:361` `opacity: 0.85`) **2.93:1**;
  `.settings-row .provider` (`:916` `var(--text-subtle)`, 10px) **3.25:1**; `.keycap`
  (`base.css:117` `var(--text-subtle)`, 10px) **3.10:1** in the header and 3.40:1 in the rail;
  `.crumb-sep` (`:1038`), `.tab-note` (`:1343`) and `.head-activity.status-idle` (`:1078`, 9.5px)
  **3.62:1**; `.path-hint` (`:184`, 10px), `.session-meta` (`:561`, 9px), `.session-group-label`
  (`:404`, 9px) and `.online-row .right` (`:819`, 9.5px) **3.80:1**; `.shortcut` (`:259`, 9.5px)
  **3.97:1**. Total: 20 of 34 visible shell text nodes below 4.5:1.
  In Ember (dark) only 3 of 34 fail (`.shortcut` 4.47:1, `.session-turns` 3.15:1, group count 3.74:1) —
  and only because the palette moved `--faint` to `#8d867e` (5.16:1 on the rail) today; **not one of
  these declarations changed**, so the dark pass is the palette's accident, not the shell's.
  Fix: the 15 text uses of `var(--faint)` → `var(--muted)` (§4 floor 4.5:1; measured **7.56:1** dark,
  **5.79:1** light on the rail); delete the `opacity: 0.7 / 0.8 / 0.85` multipliers, which cost 1.6–2.9
  points; and take every 9px/9.5px to `--fs-micro` (10px, uppercase, 0.125em) or `--fs-tiny` (11px), per
  §3.3's "9px and 9.5px are gone".

---

## Major

- **M1 — The elevation ladder's adjacent steps are 1.00–1.14:1; §3.1 asks for ≥3 L points.**
  Measured painted surfaces (Ember): canvas `#0f0e0d` L5 → project rail `#141312` L7 → sessions rail
  `#141312` L7 → inspector `#181716` L9 → surface `#23211f` L13 → raised `#2c2a28` L16 → hover `#363330`
  L20. ΔL per rung: **2, 0, 2, 4, 3, 4**; contrast per rung: **1.04, 1.00, 1.02, 1.12, 1.12, 1.14**.
  Five of the six rungs are invisible on an 8-bit panel. In Daybreak the ladder is not merely shallow,
  it **reverses**: canvas L95 → rail L97 → inspector L94 → surface L100 → raised L90 → hover L87 — the
  surface is 6 points *brighter* than the canvas it sits on and the "raised" menu is 10 points *darker*
  than the surface it opens from (measured `#ede9e0` vs `#ffffff`), so a menu reads as sunken.
  Fix: keep §3.1's ladder and consume it through the aliases (B2 fixes the rail/sidebar rung); in light
  mode make elevation move in one direction only — raised and hover must step *away* from the canvas,
  e.g. canvas `--theme-bg` L95 → surface L100 → raised L100 + `--shadow-menu`, or invert the whole
  ladder so cards darken consistently. Note for the design owner: §3.1's own table (L 5/7/8/8/11/14/17)
  does **not** satisfy the "≥3 L between neighbours" rule printed above it (bg→rail = 2, rail→sidebar =
  1, sidebar→inspector = 0); the rule and the table need reconciling or no shell can pass.

- **M2 — At 1024px the header clips its own status pill; at 1023 it disappears into a sheet.**
  `layout.css:1013` `.breadcrumb { … overflow: hidden; }` holds `.main-title` and `.head-activity`, while
  `.header-buttons` (`:1104`) is `flex: 0 0 auto` and takes 206.7px of a 438px bar.
  Measured at 1024px: `.breadcrumb` clientWidth **177px**, scrollWidth **219px** → 42px clipped;
  `.head-activity` box spans x 472.0–522.6 while the clip edge is 481.3 → **9.3 of its 50.6px visible
  (18%)**: the "ready" pill is cut mid-word. The title survives (341.4–461.0). At 1100px the crumb fits
  exactly (253/253) and at 1440px 256/256, so the break is confined to the 1024–1090px band — the first
  widths above the shell's own 1023px breakpoint.
  Fix: hide `.head-activity` in the `@media (max-width: 1100px)` block that already hides `.tab-note`
  (`layout.css:1604-1608`), or move the pill out of the `overflow: hidden` crumb into
  `.header-buttons`, which is `flex: 0 0 auto` and therefore budgeted; separate them with `--space-2`.

- **M3 — The composer dock does not line up with the transcript, and the error changes with width.**
  `layout.css:1462` — `padding: 10px 27px 12px;` against `:1439` — `padding: 30px 26px 18px;`, and
  `:1468` `.composer-inner { max-width: var(--content-max); }` against `chat.css:33`
  `.chat-inner { max-width: calc(var(--content-max) + 52px); }` (i.e. `+ 2 × 26px`, a third gutter value).
  The comment at `:1596` claims "The dock's gutter tracks the transcript's, so the box lines up with the
  conversation at every width" — measured, it does not. Offset of the composer box's left edge from the
  transcript's content left edge: **+1.0px at 1440**, 0 at 1100 and 1024, **+9.5px at 1023**, **+7.5px at
  1920**; at 1023 the composer is also 820px wide against the transcript's 824px, so the right edges
  disagree by 5.5px.
  Fix: one gutter token used by both boxes — `--space-9` (24px) per §3.2, or a named `--gutter` — and one
  max-width expression, `max-width: calc(var(--content-max) + 2 * var(--gutter))`, on `.chat-inner`,
  `.composer-inner` and `.composer-wrap`'s padding-inline.

- **M4 — Two focus cues are invisible; one rule deletes the ring outright.**
  `layout.css:1990-1994` — `.model-switcher-option:hover, .model-switcher-option:focus-visible {
  background: var(--surface); outline: none; }`. The only cue left is `#23211f` on the menu's `#2c2a28`
  = **1.12:1** (Daybreak: `#ffffff` on `#ede9e0` = 1.14:1) against §5.6's 3:1 requirement, and
  `outline: none` removes the ring base.css gives every other element.
  `layout.css:308` — `.side-search input { … outline: none; }` combined with `:292-295`
  `.side-search:focus-within { border-color: var(--line-strong); background: var(--surface-2); }`:
  focusing the field **removes its fill** (B1) and leaves a 1px `--line-strong` border, `#423f3b` on
  `#23211f` = **1.53:1** (light 1.90:1). Measured live: at rest `bg #23211f, border rgba(0,0,0,0)`;
  with focus `:focus-within` true → `bg rgba(0,0,0,0), border #423f3b`, `input:focus-visible` true and
  `outline none`. So the field looks *less* like an input when it is active.
  The ring token itself is fine: `base.css:132-136` `outline: 2px solid var(--accent)` measures
  **9.84:1** on the rail and 8.51:1 on the surface — three rules defeat it, the token does not.
  Fix: never `outline: none`; `.model-switcher-option:focus-visible { outline: 2px solid var(--accent);
  outline-offset: -2px; }` and keep `background: var(--surface-hover)` for pointer hover (1.48:1);
  `.side-search:focus-within { border-color: var(--accent); }` (8.51:1) plus the restored
  `background: var(--surface)` fill.

- **M5 — `compact` density makes one gap 6.5× larger than `comfortable`.**
  `layout.css:394-396` — `.session-group + .session-group { margin-top: 2px; }` versus `:419-421` —
  `:root[data-density='compact'] .session-group + .session-group { margin-top: 13px; }`.
  Measured by switching `data-density` on the live `<html>`: computed `margin-top` **2px comfortable →
  13px compact**, while every other compact rule *reduces*: `.side-head` padding 11/12/9 → 8/12/6
  (`:115`/`:119`), `.new-session` height 31px → 28px (`:219`/`:265`). Compact density must not enlarge a
  gap.
  Fix: comfortable `margin-top: var(--space-3)` (6px), compact `var(--space-1)` (2px) — i.e. delete the
  inverted override at `:419-421` and snap the base rule to the scale.

- **M6 — Dead and contradictory rules in these three files.**
  - `layout.css:592-634` — `.session-ratio`, `.session-ratio i`, `.ratio-add`, `.ratio-del`,
    `.session-diff`, `.diff-add`, `.diff-del`: no component renders any of them (`grep` over
    `web/src/components`: 0 hits). Six rules and a nine-line comment describing "an 18×3 two-tone ratio
    bar" that no longer exists — and two of them depend on `--surface-3` (B1), so they would be invisible
    anyway.
  - `layout.css:1005-1011` `.head-title-wrap`: never rendered.
  - `layout.css:1236-1241` `.header-divider` **and** `:1741-1743` its `display: none` at ≤1023: never
    rendered.
  - `layout.css:170-172` `.project-title-dot { display: none; }`: never rendered.
  - `base.css:267-270` `.meta-sep`: never rendered.
  - `layout.css:17-19` `--ink-secondary`, `--head-line`, `--accent-hair-hover`: defined on `.app`, **0
    consumers** in the whole stylesheet tree (1 hit each = the definition). `tokens.css:341 --bg-glow`:
    also 0 consumers.
  - `rail.css:552-562` and `:571-577` declare `.project-switcher-menu` **twice**; the second silently
    overrides `top` (`calc(100% + 4px)` → `30px`), `width` (`max-content` → `min(232px, calc(100vw -
    28px))`) and `min-width`. `rail.css:544-546` and `:564-566` declare `.project-switcher-menu-wrap`
    twice with identical bodies.
  - `layout.css:1439` `.chat-inner { padding: 30px 26px 18px; }`: the 30px top is dead — `chat.css:34`
    `padding-top: 12px` loads later (`index.css` imports `chat.css` after `layout.css`); measured computed
    padding `12px 26px 140px`.
  - `web/src/app.tsx:673` adds `is-narrow` (`class={`app${store.narrowViewport.value ? ' is-narrow' : ''}`}`);
    no stylesheet matches it (1 hit, the JSX itself).
  Fix: delete the lot, or collapse the duplicated `.project-switcher-menu` block to its second form and
  drop `top`/`width` from the first. Every removal shrinks the 260-violation surface these three files
  carry without changing a pixel.

- **M7 — Two click targets are below the 24×24 minimum.**
  `layout.css:333` `.filter-chip { … height: 20px; }` (measured 20 × 34.5 for the "All" chip, the rail's
  primary filter control); `rail.css:238-241` `.project-rail-kebab { width: 20px; height: 20px; }`
  (measured 24 × 24 only under `@media (hover: none)`, `rail.css:706-716`). `.session-menu-trigger`
  (`layout.css:661-666`) is exactly 24 × 24 and only meets it. WCAG 2.2 AA 2.5.8 wants 24 × 24.
  Fix: chip `height: 24px; padding: 0 var(--space-3)` and `--radius-sm`; kebab 24 × 24 in the base rule
  (drop the `@media (hover: none)` patch).

---

## Minor

- **m1 — Two stacked dividers, two tones.** `layout.css:998` `.main-head { border-bottom: 1px solid
  var(--line-soft) }` (`#2e2b29`, L15) sits 44px above `:1252` `.workspace-tabbar { border-bottom: 1px
  solid var(--line) }` (`#363430`, L20) — two rules in one column, 5 L points apart (1.15:1 between
  them). Fix: both `var(--line-soft)`.

- **m2 — Three left edges in the header column.** Measured at 1440: header content left **348.0** (crumb,
  `.main-head { padding: 0 26px }`, `:996`); first tab box **348.0** but its label starts at **358.0**
  (`.workspace-tab { padding: 0 10px }`, `:1277`); the active underline starts at **356.0**
  (`.workspace-tab.is-active::after { right: 8px; left: 8px }`, `:1296-1298`). The brain reads the
  label, not the box, so the tab text sits 10px right of every other header glyph. Fix:
  `.workspace-tabs { margin-left: calc(-1 * var(--space-5)); }` so the label — not the padding —
  aligns, and keep `--space-5` as the focus-ring gutter; snap `padding: 0 26px` to `--space-9` (24px)
  at the same time.

- **m3 — Off-scale font weights.** `layout.css:1492` `font-weight: 560;` and `rail.css:638`
  `font-weight: 680;`. §3.3's set is `{400, 500, 550, 600, 660, 700, 720, 800}`. Fix: 550 and 660.

- **m4 — Four different uppercase-kicker recipes.** `layout.css:405-409` `.session-group-label`
  (9px / 700 / 0.1em), `:1053-1057` `.head-activity` (9.5px / 700 / 0.07em), `:1955-1958`
  `.model-switcher-group-title` (10px / 600 / 0.06em), `base.css:242-253` `.eyebrow`
  (10px / 720 / 0.125em — the one §3.3 specifies). Fix: all four take `.eyebrow`'s `--fs-micro` / 720 /
  0.125em (and B3's contrast).

- **m5 — 27 radius declarations, 9 distinct off-scale values.** In these files: 11px (`rail.css:52`,
  `:144`), 13px (`rail.css:129`), 9px (`rail.css:363`, `layout.css:223`), 8px (`layout.css:161, 283,
  539`), 7px (`layout.css:444, 496, 903`, `rail.css:390`), 4px (`layout.css:258, 1322`, `base.css:452`),
  3px (`layout.css:878, 885`), 2px (`layout.css:462, 599, 1300`), plus three `calc()` derivations that
  land off-scale: `calc(var(--radius) - 6px)` = 4px (`base.css:116`), `calc(var(--radius) + 1px)` = 11px
  (`rail.css:327`), `calc(var(--radius) - 1px)` = 9px (`rail.css:173`). §3.4 allows 4/6/8/10/14/999.
  Fix, concretely: `.session-row` 7px (`:444`) and `.session-open` 7px (`:496`) → `var(--radius-sm)`
  (6); `.project-rail-item` 13px (`rail.css:129`) → `var(--radius-lg)` (14); `.project-rail-avatar` 11px
  (`rail.css:144`) and `.rail-tool` 9px (`rail.css:363`) → `var(--radius-md)` (8); `.filter-chip` 6px
  (`:336`) is already a legal value, so it only needs to be written as `var(--radius-sm)`;
  `.project-rail-kebab` 7px (`rail.css:243`) → `var(--radius-sm)`; `50%` anywhere it is not a circle;
  `999px` (`layout.css:1209`, `rail.css:283`) → `var(--radius-pill)`.

- **m6 — 183 spacing literals in these three files** (122 `layout.css`, 35 `rail.css`, 26 `base.css`;
  the repo total is 1092). Worst offenders, with the snap: `layout.css:996` `padding: 0 26px` →
  `--space-9` (24); `:992` `gap: 18px` → `--space-8` (20); `:1253` `.workspace-tabbar { gap: 26px }` →
  `--space-9`; `:115` `padding: 11px 12px 9px` → `--space-6 --space-6 --space-4`; `:403` `padding: 9px
  7px 4px` → `--space-5 --space-3 --space-2`; `:762` `.rail-empty { padding: 22px 14px }` →
  `--space-8 --space-7`; `rail.css:483` `gap: 13px` → `--space-6`; `:1462` the composer's 27px → the
  M3 gutter token. §3.2 caps movement at 2px, so none of these reflows.

- **m7 — The rail's bottom stack has a 6px/11px rhythm.** `rail.css:348-355` `.project-rail-bottom {
  gap: 6px }` plus `rail.css:388` `.rail-avatar { margin-top: 5px }`. Measured: 6.0px between the two
  `.rail-tool` buttons, **11.0px** to the avatar. Fix: delete the margin, `gap: var(--space-3)` (6px).

- **m8 — The "Settings" tooltip renders outside the window.** `rail.css:383-396` `.rail-avatar` has no
  `position`, while `rail.css:405-412` `.project-rail [data-tip]::after { position: absolute; top: 50%;
  left: calc(100% + 14px); }` resolves against the nearest positioned ancestor — measured, that is
  `.app-root` (`layout.css:22`, `position: relative`), so the pseudo-element computes `left: 1454px;
  top: 480px` in a 1440×960 viewport: 14px beyond the right edge, vertically centred on the screen. The
  two `.rail-tool` tooltips are correct (`left: 46px; top: 16px`) because `.rail-tool` is
  `position: relative`. Fix: `position: relative` on `.rail-avatar`; `left: calc(100% + var(--space-4))`.

- **m9 — The project kebab overhangs the rail and is clipped.** `rail.css:232-241` `top: -4px; right:
  1px; width: 20px;`. Measured box x 41.0 → **65.0**, while `.project-rail-scroll` (`overflow-x: hidden`)
  ends at **63.0** and the rail at 64.0 → 2px of the button is cut, 1px of it is outside the rail.
  Fix: `right: var(--space-2)` (4px) and 24 × 24 (M7), so the box ends at 61px.

- **m10 — The session kebab covers its own metadata.** `layout.css:655-659` `.session-menu-wrap { top:
  3px; right: 4px }` with the 24px trigger at `:661-666`; measured trigger x 288.0–312.0 against
  `.session-meta` x 80.0–308.0 → **20px of overlap**, in the same 12px-tall line that carries "1 turn".
  At rest the trigger is `opacity: 0` (`:664`), so the text is legible until hover, when the button paints
  over it. Fix: reserve the slot — `.session-open { padding-right: calc(24px + var(--space-2)); }` (i.e.
  `--space-9` + `--space-1` = 26px), or give `.session-meta { padding-right: var(--space-9) }`.

- **m11 — The brand tile's glyph is below even the decorative floor in light mode.** `rail.css:62`
  `color: var(--on-accent);` over the gradient at `:53-58` built from `--accent-fill`. Measured
  Daybreak: ink `#ffffff` on the darkest stop (`color-mix(in srgb, var(--accent-fill) 62%, var(--bg))` =
  `#be8f74`) = **2.85:1** (midpoint 5.83, lightest stop 9.96); Ember: 4.64:1 at the same stop. §4's
  decorative floor is 3.0:1. The glyph is decorative (the button's name comes from
  `aria-label="Open control plane"`, `ProjectRail.tsx:472`), so this is not an AA failure — it is a mark
  that disappears at one end. Fix: flatten the tile to `background: var(--accent-bg); color: var(--accent);`
  (a pair measured at 6.46:1 in Ember) or clamp the gradient's darkest stop to `--accent` so the ink is
  never below 4.5:1.

- **m12 — Ten of these files' 50 font-size literals sit at 10.5px, a size with no token at all.**
  `base.css:540, 545`, `layout.css:716, 931, 1829, 2024, 2035`, `rail.css:465, 471, 678`. §3.3 publishes
  nothing between `--fs-micro` (10px) and `--fs-tiny` (11px), so 10.5px cannot be expressed; it is also
  outside the 10px/11px floors' intent (a kicker or dense metadata). Snap each to `--fs-micro` or
  `--fs-tiny` — 10 of the 26 sub-11px declarations in these files are this one value.

---

## The five I would fix first

1. **B1** — define `--surface-2` / `--surface-3` (`tokens.css:334-335`). One line, and the selected
   session row, every hover fill, the search focus fill, the context meter and the project tiles come back.
2. **B2** — `.rail { background: var(--sidebar) }` (`layout.css:40`). One line separates the two left
   columns from 1.00:1 to 1.06:1.
3. **B3** — the 15 text uses of `--faint` → `--muted`, drop the 0.7/0.8/0.85 multipliers, take 9–9.5px to
   `--fs-micro`/`--fs-tiny`. This is the surviving half of §1.3 and the only thing that fails AA broadly.
4. **M4** — stop deleting the focus ring (`layout.css:1990-1994`, `:308`) and put a 3:1+ cue on the
   search field and the menu option; the `--accent` ring already measures 9.84:1.
5. **M2** — hide `.head-activity` at ≤1100px so the header stops clipping its own status pill at 1024px.

Immediately after: **M3** (one gutter token for the transcript and the dock — it is a 1px bug at 1440 and
a 9.5px bug at 1023, so it will keep being re-reported until there is one number).
