# Overlays and settings — stylesheet audit

Domain: `web/src/styles/overlays.css`, `settings.css`, `theme-studio.css`, `notifications.css`
and the components that paint them (`Dialog`, `CommandPalette`, `ToastRegion`, `RewindDialog`,
`FileMentionMenu`, `NotificationCenter`, `SettingsSheet`, `ThemeStudio`, `ModelSwitcher`).
Every finding below is judged against `docs/internal/DESIGN-SYSTEM.md`; the section numbers
(§3.1, §5.5, …) refer to it.

## Verdict

The overlay family is where §1's diagnosis is still legible in full: these four files carry
**445 of the 1,693** static violations `design-audit.mjs` reports (107 font-size, 36 radius, 301
spacing, 1 raw colour) from **24.4%** of the CSS lines in `src/styles` — proportionally no worse
than the rest of the app, which is the point: a quarter of the drift lives here, in five distinct
sub-11px type sizes (8.5 / 9 / 9.5 / 10 / 10.5px) and five hand-authored shadows. Dark mode has
been largely repaired by the in-flight token migration — in Ember every overlay text role I
measured now clears 4.5:1 (the palette keycap at 4.03:1 is the exception) and the ladder is
monotonic — but
light mode has not been touched, and it fails exactly as §1 predicts: on Daybreak the dialog
fill sits **3.5 L points *below* the canvas** (1.066:1), the toast **0.4 points above it**
(1.009:1), the 67%-black scrim drags a 95.5%-L canvas down to 31.5% L, the palette's
keyboard-selected row reads at **3.61:1**, the settings sheet's primary button at **4.00:1** and
the notification timestamp at **3.62:1**. Underneath the colour sits a structural problem: one
modal family authors five shadows against a documented budget of two, uses two scrims and three
fills for one layer, the Theme Studio's `z-index: 110` is inert because it is never portalled,
and nothing prevents two `aria-modal="true"` surfaces from being open at the same time.

**Measurement basis.** Live build at `http://127.0.0.1:8943` (the working tree of
`/home/karutoil/karucode`, whose token layer was mid-migration when measured), Chromium,
1440×960, Ember dark and Daybreak light. The four audited stylesheets are byte-identical
between that tree and the audit worktree, so every `file:line` below holds; the *token values*
are the ones in the working tree, not the committed `tokens.css`. Contrast is WCAG relative
luminance against the composited background; "L" is HSL lightness percent, the same column
§3.1 uses. Values marked *(dark)* are Ember, *(light)* are Daybreak.

---

## Blockers

### B1 — Overlay surfaces are not on the ladder, and in light mode the ladder is inverted

- **`overlays.css:33`** — `.dialog` (palette, catalog, rewind, file viewer, notification centre):
  `background: var(--surface-menu);`
  Evidence *(dark)*: `#2f2c2a`, L **17.5%**, against `--theme-bg` L **5.5%** (+12.0 points).
  Evidence *(light)*: `#f0ede5`, L **92.0%**, against `--theme-bg` `#f6f4f1` L **95.5%** — the
  dialog is **3.5 points darker than the page it floats over**, contrast **1.066:1**. Elevation
  is carried entirely by `--shadow-overlay`; the fill does no work.
- **`overlays.css:312`** — `.dialog-sheet` (the whole settings sheet): `background: var(--sidebar);`
  Evidence *(dark)*: L **9.8%** — the *third* rung, the same fill as the session list, while the
  app's own cards sit at `--theme-surface` L **12.9%**; opening settings therefore shows a panel
  darker than the cards behind it. *(light)*: L **92.9%**, **1.051:1** against the canvas.
- **`overlays.css:719`** — `.toast`: `background: var(--surface-toast);`
  Evidence *(dark)*: L **19.4%** ✓. *(light)*: `#f7f5f2` L **95.9%** against the canvas's
  **95.5%** → **1.009:1**: a toast is invisible against the page except for its shadow.
- **`notifications.css:24`** — `.notification-toolbar`: `background: var(--surface);` inside a
  `--surface-menu` dialog. Evidence *(dark)*: the toolbar is **4.6 L points darker** than its own
  dialog (a sunken band in a popover); *(light)*: `#ffffff` L **100%** is **8.0 points brighter**
  (a raised band). One declaration, opposite depth cues per mode: the two rungs cross.
- **Fix:** every overlay fill takes the §3.1 rung the contract names for menus,
  **`var(--theme-raised)`**, and `--surface-menu` / `--surface-toast` are retired as separate
  rungs. This only works after Daybreak is re-derived: today `--theme-raised` (`#ede9e0`, L
  90.4%) is **5.1 points below** `--theme-bg`; §3.1 requires each step **≥3 L points above** its
  neighbour. Use `--theme-surface` for the scrub/toolbar bands (a dialog may not contain a rung
  above its own), or drop the band and separate with `--line`.

### B2 — Two scrims, both an absolute-black literal, and neither inverts for light mode

- **`overlays.css:22`** — `.dialog-backdrop`: `background: color-mix(in srgb, #000 67%, transparent);`
  and **`:23`** `backdrop-filter: blur(4px);`. This is the file's only raw colour (§5.1) and it is
  the darkest surface in the product.
- **`theme-studio.css:18`** — `.theme-shade`: `background: var(--scrim);` = `rgb(3 4 6 / 60%)`
  with `backdrop-filter: blur(3px)`.
  Evidence: same job, **67% vs 60%** and **blur 4px vs 3px**. Measured in light mode, the canvas
  `rgb(246 244 241)` (L **95.5%**) becomes `rgb(81 81 80)` (L **31.5%**) behind a dialog — a
  **7.3:1** luminance collapse — and `rgb(100 100 100)` (L **39.3%**, 5.4:1) behind the drawer.
  The identical scrim in Ember only moves the canvas from `rgb(15 14 13)` to `rgb(5 5 4)`, which
  nobody notices. So the light theme is dimmed with a dark theme's scrim: the app behind a modal
  is *more* separated than the modal itself.
- **Fix:** delete the `#000 67%` declaration and let **both** layers read `var(--scrim)`. Then
  make `--scrim` mode-derived rather than fixed — `--scrim` is an `alpha` role anchored to `bg`
  in `roles.ts:192` but its value is a literal — so Daybreak gets ≈`rgb(15 23 42 / 18%)`-class
  strength (the alpha `--shadow-overlay` already uses in light mode) and Ember keeps 55–60%.
  Blur must come from one value as well; `blur(4px)` behind a `.dialog` and `blur(3px)` behind a
  drawer is two answers to a question §5.5 says has one.

### B3 — Five shadows in one modal family; the sanctioned `--shadow-menu` is unused by every overlay

§5.5 allows exactly two shadows and forbids authoring a third. In this family:

| Surface | Declaration | Resolves to (light / dark) |
| --- | --- | --- |
| `.dialog` `overlays.css:34` | `var(--shadow-overlay)` | `0 30px 90px rgb(15 23 42 / 18%)` / `0 30px 90px -24px rgb(0 0 0 / 85%)` ✓ |
| `.dialog-sheet` `overlays.css:313` | `-22px 0 90px var(--shadow-lg)` | geometry hand-authored; `--shadow-lg` is the *colour* `rgba(15 23 42, .16)` |
| `.toast` `overlays.css:725` | `0 20px 60px var(--shadow-lg)` | same |
| `.state-card` `overlays.css:807` | `0 25px 90px var(--shadow-lg)` | same |
| `.theme-studio` `theme-studio.css:34` | `-22px 0 90px var(--theme-shadow)` | same, via the private token |

- **`overlays.css:313`, `:725`, `:807`, `theme-studio.css:34`** — four hand-authored geometries.
  Evidence: **three** different shadow tokens (`--shadow-overlay`, `--shadow-lg`,
  `--theme-shadow`) plus four literal offset/blur pairs; `0 20px 60px`, `0 25px 90px` and
  `-22px 0 90px` are three distances for one elevation step. `--shadow-menu` exists, is correct,
  and is referenced only in `composer.css:724` and `:930` — no overlay uses it.
- **Fix:** `.dialog-sheet` and `.theme-studio` → `var(--shadow-overlay)` (a drawer is an overlay,
  not a menu); `.toast` → `var(--shadow-menu)`; `.state-card` → `var(--shadow-overlay)`.
  `--shadow-lg` must stop being used as a shadow — it is a colour, and using it in the shorthand
  is what makes each of these look like a new token.

### B4 — In light mode the accent is used as ink and misses the §4 floor, including the sheet's primary action

`--accent` in Daybreak is tuned to be a *fill* (`#bb5f2c`, 4.00:1 on `--theme-bg`); §4 requires
**4.5:1** for `accentText`. The token that satisfies it exists: `--theme-accent-text` `#9b5128`
measures **5.31:1** on the canvas, and the documented fill pair `--accent-fill` (`#9b5128`) with
`--on-accent` (`#ffffff`) measures **5.83:1**. The overlays use the wrong half of the pair.

- **`base.css:310`** — the settings footer's primary button (`button-primary`): `background: var(--accent); color: var(--bg);` → measured **4.00:1** on Daybreak ("Save changes", `settings.css:1264`). §4 requires 4.5.
- **`theme-studio.css:934`** — `.theme-btn.primary`: `background: var(--accent); color: var(--on-gold);` → **4.00:1**. It also uses `--on-gold`, a fidelity-role token, instead of `--on-accent`.
- **`overlays.css:210`** — `.palette-item:hover, .palette-item.is-active` (`:212`–`:213`): `background: var(--row-selected-bg); color: var(--row-selected-text);` → **3.61:1**. This is the row **Enter will run**; it is the palette's primary affordance and it is below AA in light mode (`--row-selected-text` anchors to `accent` at `roles.ts:109` and must anchor to `accentText`).
- **`theme-studio.css:120`** — `.theme-top-eyebrow`: `color: var(--accent);` → **3.80:1**; **`:217`** `.theme-mini-action`: `color: var(--accent);` → **3.80:1** — a *button label* ("New theme", "Reset colours", "Copy variables").
- **`theme-studio.css:131`** — `.theme-head-badge`: `color: var(--accent)` on `--accent-bg` → **3.61:1**.
- **Fix:** accent-as-ink → `var(--theme-accent-text)` (add the public alias `--accent-text`; tokens.css publishes only the private name); accent-as-fill → `background: var(--accent-fill); color: var(--on-accent)`. For `.palette-item.is-active` keep `--row-selected-bg` and take `--theme-accent-text` for the ink (5.31:1) — or fix the anchor in `roles.ts:109`.

### B5 — `--faint` and `--text-subtle` used as readable text: 2.91–3.62:1 in light mode

§4: `--faint` is **decorative only, 3.0:1 floor**, "never for text a user must read". Daybreak's
`--faint` is `#949088` (2.84:1 on the canvas) and `--text-subtle` is `#a39c90` (2.48:1 on
`--theme-sidebar`).

- **`overlays.css:293`** — `.palette-footer kbd { color: var(--faint); }` → **3.28:1** (*light*); **4.03:1** (*dark*, below the 4.5 AA floor). These keycaps are the palette's only instructions ("↑↓ navigate", "Enter run", "Esc close").
- **`overlays.css:233`** — `.palette-esc { color: var(--icon-pill-down); }` → **2.91:1** (*light*), **4.53:1** (*dark*).
- **`notifications.css:166`** — `.notification-time { color: var(--faint); }` → **3.62:1** (*light*); a timestamp the user reads to order the list.
- **`overlays.css:1122`** — `.rewind-file-meta { color: var(--text-subtle); }` → **3.10:1** (*light*) — the per-file `+12 −3` counts in the rewind confirmation.
- **Fix:** all four take `var(--muted)` (5.25:1 light / 6.5–7.7:1 dark) and, where the element is genuinely decoration (the `↑`/`↓` glyphs), the glyph keeps `--faint` while the label word is `--muted`. Do not fix this by lightening `--faint`: §4 pins it at 3.0.

### B6 — The Theme Studio's `z-index: 110` is inert; two modals can be open at once

- **`theme-studio.css:11`** — `.theme-studio-layer { position: fixed; inset: 0; z-index: 110; }`
  renders **in place**, inside `.app-root` → a wrapper with `isolation: isolate`. That ancestor is
  a stacking context at `z-index: auto`, so the 110 is confined inside it, while every `Dialog`
  portals straight into `document.body` at `overlays.css:13` `z-index: 100`.
  Evidence: reproducing the plain user path **Settings → Appearance → "Theme studio" → Ctrl+K →
  "Open settings"** leaves **two** `[aria-modal="true"]` nodes alive (`theme-studio` and
  `dialog-sheet dialog-settings`); `document.elementFromPoint` over the *centre of the drawer*
  returns `.dialog-backdrop`, i.e. the older modal is fully buried under the newer one's scrim
  (the two scrims composite to **86.8%** black) yet keeps its focus trap and its
  `aria-modal` claim. Separately, with only the drawer open, Ctrl+K puts the palette **above** the
  drawer at `z-index: 100 < 110`.
- **Fix:** render `ThemeStudio` through the same portal `Dialog` uses
  (`createPortal(…, document.body)`), and declare the scale once in `tokens.css`
  (`--z-dialog: 100; --z-drawer: 110; --z-toast: 130`) instead of four literals in four files
  (`overlays.css:13`, `:702`, `theme-studio.css:11`, `layout.css:1920`). Opening a modal should
  also close the previous one — the app currently has no single-modal owner, which is why the
  buried dialog can outlive the user's intent.

---

## Major

### M1 — The command palette's only input has no focus ring

- **`overlays.css:156`–`:164`** —
  `.palette-input:focus { outline: none; box-shadow: none; }` and
  `.palette-input:focus-visible { border-radius: 4px; outline: none; }`
  Evidence: with the input focused, computed `outline-style: none`, `box-shadow: none`; the whole
  indicator is `overlays.css:169` `box-shadow: inset 0 -1px 0 var(--accent)` on
  `.palette-search:focus-within` — a **1px underline** where `base.css:132` provides a 2px ring
  plus a 4px glow. §5.6 requires a visible ring on every interactive element.
- **Fix:** delete both `outline: none` / `box-shadow: none` blocks and let the base ring apply;
  keep the underline as an extra. §5.6 names `--focus-ring`, which **does not exist** in
  `tokens.css` — add `--focus-ring: var(--accent)` (accent measures 3.89:1 against
  `--surface-menu` in light and 7.6:1 in dark, so it clears the 3.0 floor in both modes) and use it
  in all eight focus rules in this domain.

### M2 — Half the domain's type is below the 11px floor, in five off-scale sizes

- **`theme-studio.css:759`** — `.theme-preview-thought.is-collapsed::after { font-size: 8.5px; }`
  Evidence: measured `8.5px` — the smallest type in these four files.
- **`overlays.css:1097`** — `.rewind-file-action { font-size: 9px; }`; **`theme-studio.css:128`**
  `.theme-head-badge { font: 9px var(--font-mono); }`; **`:365`** `.theme-preset-mode { font: 9px … }`.
- **`overlays.css:190`**, **`:295`**, **`settings.css:1055`**, **`:1063`**, **`theme-studio.css:702`**,
  **`:711`**, **`:734`**, **`notifications.css:138`** — 9.5px (eight declarations).
- **`settings.css:175`**, **`:254`**, **`:633`**, **`:642`**, **`:649`**, **`:977`** and
  **`theme-studio.css:218`**, **`:504`**, **`:559`**, **`:583`**, **`:655`**, **`:803`**, **`:868`**
  — 10.5px (fifteen declarations), a value that appears in no scale.
  Evidence: **54 of the 107** literal font sizes in these four files are under 11px, across five
  distinct values (8.5 / 9 / 9.5 / 10 / 10.5); `theme-studio.css` is the worst at **19 of 29**.
  The audit script cannot see the `var(--fs-micro)` uses, so the true sub-11px surface is larger
  (most of `overlays.css:971`–`1145` is 10px prose).
- **Fix:** prose never goes below **`--fs-tiny` (11px)**; `--fs-micro` (10px) is reserved for
  uppercase kickers, which is the only place 9/9.5px may have come from. `--fs-tiny`,
  `--fs-label`, `--fs-lead` are **not in `tokens.css`** today — they must land before this file
  can be converted.

### M3 — The settings sheet has three vertical rhythms and no single control alignment

- **`settings.css:148`** — `.settings-section { padding: 20px 0 22px; gap: 14px; }` → **43px**
  between top-level sections (measured section tops 132 → 446).
- **`settings.css:1199`** — `.settings-advanced-body { padding: 0 13px 6px; }` and
  **`:1204`–`:1210`** `padding-bottom: 14px` / `:first-child padding-top: 14px` → **29px** one
  level in (measured top 698).
- **`settings.css:221`** — `.settings-subhead { margin-top: 20px; padding-top: 16px; }` → a third
  rhythm, **36px**.
  Evidence: three spacings (43 / 36 / 29) for the same job inside one 771px-tall panel; none of
  the three is on the §3.2 scale, whose largest step is 32px.
- **`settings.css:166`** — `.settings-section-head h3 { font-size: 13px; }` vs
  **`:266`** `.setting-name { font-size: 12px; }` — the section title is **1px** larger than the
  row label beneath it, and the same 13px as `--fs-body` prose. The hierarchy is a 1px step.
- Controls in one panel, measured: heights **37 / 32 / 30 / 16** (`panel-action-btn`, `.input`,
  `.button`, `.input-range`) and right edges **1427 / 1113 / 985** — the disclosure's own grid is
  inset by **14px** (`settings.css:128` panel gutter 23px vs `:1199` disclosure gutter 13px), so
  two selects in the same column end at 1427 and 1413.
- **Fix:** sections `padding: var(--space-9) 0 var(--space-8)` (24 / 20 → 44px, +1px, inside §3.2's
  2px budget) and the same inside disclosures; `.settings-subhead` becomes `margin-top: var(--space-8)`
  with no padding-top; `.settings-section-head h3` → **`--fs-lead` (14px)** so it clears
  `--fs-label` (12px) by 2px; one control height (`--space-10`, 32px) for `.input`,
  `.button` and `.panel-action-btn`; one gutter (`--space-8`, 20px) for panel and disclosure so
  every grid shares a right edge.

### M4 — Two dialog shells for one role, with different radii, borders, fills, motion and geometry

- **`overlays.css:26`–`:37`** (`.dialog`) vs **`overlays.css:301`–`:315`** (`.dialog-sheet`).
  Evidence: radius **10px vs 0**; border **1px all round `--line-overlay` vs 1px left only
  `--line-strong`**; fill `--surface-menu` vs `--sidebar`; animation **`dialog-in` 160ms
  fade+rise vs `slide-in-right` 200ms**; centred `place-items: center` vs `position: fixed`
  right-anchored `100dvh`; `.dialog-settings` is *both* (`settings.css:15` widens it to 680px).
  The palette, rewind, catalog and file viewer are centred cards; the settings sheet and the
  notification centre (`.notification-center`, a `.dialog`) are drawer-adjacent, so the two
  layers the user opens most often move in from different directions at different speeds.
- **Fix:** one `.dialog` primitive with a documented variant (`--radius`/`--shadow-overlay` for
  both; a drawer variant differs only in `inset` and slide direction), both portalled and both
  animated with `var(--transition)`. The line tone collapses to `--line` (§3.5: three line tones
  total; this file alone uses **ten**: `--line`, `--line-strong`, `--line-card`, `--line-keycap`,
  `--line-preview`, `--line-toast`, `--line-hair`, `--line-overlay`, `--line-divider`,
  `--line-card-strong`).

### M5 — A text input is painted with the rail, not the input rung: the same field is a hole in settings and a card in the composer

- **`base.css:421`** — `.input { background: var(--rail); }`, inherited by every settings field
  through the `.input` sizing rules at `settings.css:362`, `:482`, `:675`, `:764` and the catalog
  search.
  Evidence *(dark)*: the settings input is `#141312`, L **7.5%** — **2.3 points darker than the
  sheet** (L 9.8) and **6.8 points darker than the composer's field**, which uses
  `--surface-input` `#272422`, L **14.3%** (`composer.css:39`). The same control is a hole in one
  place and a raised box in the other, with borders `--line` vs `--line-input`. *(light)* the two
  happen to agree (L 97.5 vs 96.9), which is why the defect only shows in the default theme.
- **Fix:** `.input` takes **`var(--surface-input)`**, with `--line-input` as its border, so one
  input rung exists. Keep `--rail` for the rail.

### M6 — A dialog opened over a dialog: same z-index, double scrim, and a child centred on the viewport instead of over its parent

- **`overlays.css:13`** — `.dialog-layer { z-index: 100; }` for *every* layer.
  Evidence: Settings → Models → "Browse models.dev" produces **two** `.dialog-layer`s both at
  `z-index: 100`, each with `background: color-mix(#000 67%)`, each with
  `backdrop-filter: blur(4px)`; they composite to **89.1%** black over **two** blur passes, so the
  parent drawer is erased rather than dimmed. The parent occupies x **760–1440**, the child
  x **440–1000** — **320 of the child's 560px (57%) hangs outside the dialog that owns it**, over
  the app canvas. The child's fill (`--surface-menu`, L 92.0 light) is **0.9 L points darker** than
  its parent's (`--sidebar`, L 92.9) — closer to the canvas than the layer it is stacked on, so
  the thing "on top" looks further away.
- **Fix:** sibling dialogs stack in DOM order only by accident; give a nested dialog
  `--z-dialog + 1` and **one** scrim (the topmost layer owns the scrim; inner layers get none) —
  §5.5's two shadows and one depth model. Centre a child over its parent's box
  (`place-items: center` inside the parent's rect) or make it a full-screen takeover; a
  half-outside modal is neither.

### M7 — 36 off-scale radii, including the modal family's own close buttons

- **`overlays.css:88`** — `.dialog-header .icon-button { border-radius: 9px; }`;
  **`theme-studio.css:156`** `.theme-close { border-radius: 9px; }` (and `:390`, `:421`, `:550`,
  `:767` also 9px); **`theme-studio.css:250`** `.theme-preset { border-radius: 11px; }` (and
  `overlays.css:1096` `999px` written out instead of `--radius-pill`);
  **`theme-studio.css:220`**, **`:303`** `5px`; **`settings.css:107`** `border-radius: 3px 3px 0 0`.
  Evidence: **36** of the domain's **68** radius declarations are off-scale; `theme-studio.css`
  alone accounts for **20** of its own **23**. The off-scale values used here are 3, 5, 7, 9 and
  11px, plus `999px` written out and a two-corner `3px 3px 0 0`.
- **Fix:** §3.4's six values — 9px → `--radius-md` (8px), 11px → `--radius` (10px), 5px →
  `--radius-sm` (6px), `999px` → `--radius-pill`, `3px 3px 0 0` → `--radius-xs --radius-xs 0 0`.
  `--radius-md` and `--radius-xs` are missing from `tokens.css`.

### M8 — A dead token with a dead fallback hides a missing surface

- **`overlays.css:964`** — `.rewind-target { background: var(--surface-1, transparent); }`
  Evidence: `--surface-1` is defined **nowhere** in `tokens.css` or `roles.ts`; the computed
  background is `rgba(0, 0, 0, 0)`, so the quoted message the rewind dialog is *about* has no
  surface at all — only a 2px `--line-strong` left rule. This is §1.5's `var(--warn, #d09a3c)`
  pattern in a different file: a fallback that hides a typo, invisibly.
- **Fix:** `background: var(--theme-surface)` (the quote sits *in* the dialog, one rung down) and
  delete the fallback — a missing token should render nothing so a test catches it.

---

## Minor

- **`settings.css:507`** — `.toggle { background: var(--toggle-track); }` with
  **`:521`** `.toggle-knob { background: var(--toggle-knob); }`: the off knob measures
  **2.68:1** against its own track (*dark*), and the off track measures **1.4:1** against the
  sheet (*light*). A 36×21 control with `border: 1px solid transparent` (`:505`), a knob at 2.68:1
  and a track at 1.4:1 does not read as a control (§4's 3.0 floor applies to the ring; a boundary
  is the same problem). Fix: track `--line-strong`, knob `--muted`, and
  `border: 1px solid var(--line-strong)` when off.
- **`overlays.css:762`** — `.toast-close { width: 22px; height: 22px; }` and **`:229`**
  `.palette-esc { height: 18px; }`, **`notifications.css:51`**
  `.notification-toolbar-actions .button { height: 26px; }`: three interactive targets under the
  24×24 minimum in one domain, while `.button` elsewhere is 30px. Fix: 24px minimum, 30px for
  anything with a label.
- **`notifications.css:219`** — `.notification-mark { opacity: 0; }` stays hit-testable
  (`pointer-events` is not disabled): a 26×26 invisible button sits at the right edge of every
  unread row, so a click meant for the row marks it read. Fix: `visibility: hidden` +
  `:hover/:focus-within { visibility: visible }`, or reserve the space and always show it at
  `--muted`.
- **`theme-studio.css:940`** — `.theme-btn.primary:hover:not(:disabled) { filter: brightness(1.08); }`
  is a hand-mixed state (§5.4): every other hover in the domain moves a token
  (`--surface-hover`, `--accent-line`). Fix: `background: var(--accent-fill)` → a documented hover
  rung, not a filter on top of it.
- **`settings.css:1174`/`:1179`** — `transition: transform var(--transition-fast)` on
  `.settings-advanced-label svg` and `transform: rotate(180deg)` when open: a 120ms rotation that
  is **not** in the reduced-motion block at `:1350`, which lists only `.toggle`, `.toggle-knob`
  and `.panel-action-btn`. Everything else in the domain is covered (`overlays.css:931` kills the
  three overlay entrances, `theme-studio.css:960` the drawer and the preset lift). Fix: add the
  chevron to the block.
- **`theme-studio.css:936`**, **`:138`**, **`settings.css:785`**, **`:1103`**, **`:603`**,
  **`overlays.css:817`**, **`theme-studio.css:919`**, **`:353`**, **`:781`**,
  **`settings.css:1169`** — font weights **750 ×1, 710 ×2, 650 ×5, 620 ×2** are outside §3.3's set
  `{400, 500, 550, 600, 660, 700, 720, 800}`; ten declarations, each a one-off.
- **`overlays.css:192`**, **`settings.css:786`** `letter-spacing: 0.9px`;
  **`theme-studio.css:118`** `1.5px`; **`overlays.css:990`/`:1038`**, **`settings.css:1056`**
  `0.04em`; **`overlays.css:1099`** `0.05em`; **`settings.css:620`**, **`notifications.css:140`**
  `0.06em`; **`settings.css:1104`** `0.07em` — the uppercase-kicker role has **six** spacings in
  this domain and none is §3.3's `0.125em`. Fix: one `.eyebrow` (10px / 720 / 0.125em) used by
  `.palette-group-label`, `.theme-top-eyebrow`, `.provider-add-title`, `.diagnostics-status`,
  `.rewind-modes legend`, `.notification-kind`.
- **`overlays.css:210`** — `.palette-item:hover, .palette-item.is-active` share one rule, so the
  row under the pointer and the row Enter will run are the same accent fill (6.46:1 dark). Fix:
  hover `--surface-hover`, selected `--row-selected-bg` — the user must be able to see which of
  the two Enter acts on.
- **`notifications.css:71`–`:76` and `:107`–`:109`** — `.notification-row` is declared twice, the
  second time only to add `border-left: 2px solid transparent`; the tone tints then fight
  `.is-read`'s `border-left-color: transparent` at `:201`. Fix: one rule, one tone table.
- **`overlays.css:1096`** — `.rewind-file-action { padding: 0 6px; border-radius: 999px; font-size: 9px; }`
  with a `color-mix(… 14%, transparent)` background: a pill whose fill is a hand-mixed tint of
  `--ok`/`--warn` — the only place in the domain that invents a status tint instead of using
  `--success-bg` / `--danger-bg`. Fix: the documented status pairs.
- **`web/src/components/CommandPalette.tsx:202`** — `.palette-group-label` carries
  `aria-hidden="true"` while the listbox is `role="listbox"`: the group names that carry the
  palette's whole visual hierarchy ("Actions" / "Sessions") are not in the accessibility tree.
  Fix: `role="group"` + `aria-label`, or `aria-labelledby` on the option group.
- **`overlays.css:71`** — `.dialog-title { font-size: 18px; }` (§3.3 reserves 18px for *section
  headings*; a dialog title is `--fs-lead`, 14px, or `--fs-title`, 16px) and **`:79`**
  `.dialog-description { font-size: 11px; }` (`--fs-tiny`). The sheet repeats both at
  `overlays.css:335`/`:342`. One title size for one role.

---

## The five I would fix first

1. **B2 — one scrim, derived per mode.** Delete `overlays.css:22`'s `#000 67%`, make `--scrim`
   mode-derived, and use it for `.dialog-backdrop` and `.theme-shade` alike. This is the single
   change that stops Daybreak reading as a blackout (canvas L 95.5% → 31.5% today, target ≈ 88%)
   and it removes the file's only raw colour.
2. **B4 — point the accent at the right half of the pair.** `--theme-accent-text` for ink
   (5.31:1 vs today's 3.61–4.00), `--accent-fill`/`--on-accent` for fills (5.83:1 vs 4.00). Four
   declarations in this domain, one anchor change in `roles.ts:109`, and the palette's selected
   row and the settings' primary button come back above AA in light mode.
3. **B1 — put the overlays on `--theme-raised` and re-derive Daybreak's ladder.** Until the
   light ladder descends in the right direction, no component fix can make a menu legible as
   "above" the page (today: dialog L 92.0 vs canvas 95.5, toast 95.9 vs 95.5).
4. **B6 — portal the Theme Studio and give the overlays one z-scale.** `z-index: 110` that does
   not win is worse than no z-index: it hides a live `aria-modal="true"` drawer behind a palette.
5. **B3 — delete the four hand-authored shadows.** `.dialog-sheet` / `.theme-studio` →
   `--shadow-overlay`, `.toast` → `--shadow-menu`, `.state-card` → `--shadow-overlay`; then
   `--shadow-lg` stops being a shadow anywhere and §5.5's "do not author a third" becomes
   checkable by grep.