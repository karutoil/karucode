# karucode brand guide

The identity is a **k** built from two strokes: a vertical bar (the prompt cursor)
and a chevron (the wedge that turns it into a k, and reads as `>`). Warm charcoal
and gold carry it, which is the palette the web UI already uses.

```
bar   #eae9e5 on dark   /  #111213 on light     chevron  #dfb680  (gold)
```

## What is where

| Path | What it is |
| --- | --- |
| `logo/mark.svg` | the mark on a dark surface (paper bar, gold chevron) |
| `logo/mark-on-light.svg` | the mark on a light surface (ink bar, deep gold chevron) |
| `logo/mark-mono-black.svg`, `mark-mono-white.svg` | one-colour versions for stamps, embossing, favicons on unknown backgrounds |
| `logo/lockup-*.svg` | mark + wordmark, four variants (dark, light, mono black, mono white) |
| `icons/app-icon.svg` | 1024px master, rounded tile, subtle inner hairline |
| `icons/app-icon-maskable.svg` | full-bleed for Android adaptive icons (safe zone = 66% centre) |
| `icons/favicon.svg` | small-size mark with heavier strokes, what browsers get |
| `social/banner.svg` | README banner, 1280×320 |
| `social/social-card.svg` | Open Graph / share card, 1280×640 |
| `tokens/brand.css`, `tokens/brand.json` | brand primitives as CSS custom properties and JSON |
| `scripts/build_svgs.py` | regenerates the text-bearing SVGs with the wordmark **outlined to paths** |
| `scripts/render.py` | renders every PNG/ICO and copies the web assets into `web/public/` |
| `fonts/` | Inter and JetBrains Mono (SIL OFL) — needed only to outline text |
| `directions/index.html` | the four candidate directions this was chosen from, for the record |
| `png/` | generated output (not committed) |

Everything under `web/public/` (`favicon.svg`, `favicon.ico`, `apple-touch-icon.png`)
is produced by `render.py`; edit the SVGs here and re-render instead of touching them.

## Regenerating

```bash
python3 -m venv /tmp/kcvenv && /tmp/kcvenv/bin/pip install fonttools uharfbuzz pillow
/tmp/kcvenv/bin/python branding/scripts/build_svgs.py   # outline the text
/tmp/kcvenv/bin/python branding/scripts/render.py       # PNG/ICO + web/public
```

`build_svgs.py` exists because SVG `<text>` renders in whatever font the viewer
happens to have — on a machine without Inter the wordmark silently becomes Arial
and overflows the lockup. Outlining makes every asset render identically everywhere.

## Clear space and minimum size

* Clear space on all sides: **half the mark's height** (24 units on the 48-unit
  mark). Nothing — text, rules, other logos — enters that box.
* Minimum size: **16px** for the favicon variant, **24px** for `mark.svg`. Below
  that, the gold chevron loses contrast against charcoal; use the mono version.
* Minimum lockup width: **120px**. Under that, use the mark alone.
* Never: stretch, rotate, recolour the chevron to a non-palette hue, add a drop
  shadow, place the gold mark on a gold or mid-grey surface, or set the wordmark
  in a second typeface.

## Colour

Brand primitives live in `tokens/brand.css`. The web UI keeps its own semantic
layer (`web/src/styles/tokens.css`) — these are for docs, sites and print.

| Token | Hex | Use |
| --- | --- | --- |
| `--kc-ink-950` | `#111213` | default background |
| `--kc-ink-900` | `#141516` | rail |
| `--kc-ink-850` | `#18191a` | sidebar |
| `--kc-ink-800` | `#1c1d1f` | raised surface |
| `--kc-ink-700` | `#2a2c2f` | hairline |
| `--kc-paper` | `#eae9e5` | primary text on dark — 15.4:1 |
| `--kc-paper-2` | `#b4b3ae` | secondary text on dark — 8.9:1 |
| `--kc-muted` | `#8b8d8c` | tertiary text on dark — 5.6:1, the floor |
| `--kc-light` | `#f6f4ef` | light background |
| `--kc-gold` | `#dfb680` | accent on dark — 10.0:1 |
| `--kc-gold-hi` | `#f1c691` | hover |
| `--kc-gold-deep` | `#8a5d24` | accent on light — 5.2:1 |
| `--kc-gold-bg` | `#32291e` | accent wash |
| `--kc-gold-line` | `#58452e` | accent border |
| `--kc-green` | `#88baa0` | success / added |
| `--kc-red` | `#e49e91` | danger / removed |
| `--kc-blue` | `#92b3d8` | info |

Gold is an **accent**, not a fill: it marks the thing to act on (the send button,
the active project, the chevron). A gold surface behind gold text is never
allowed. On light surfaces substitute `--kc-gold-deep`; `#dfb680` on paper is
1.9:1 and fails.

## Type

* **UI and wordmark: Inter.** Wordmark is Inter Bold with `-0.04em` tracking, set
  in sentence case as `karucode` — one word, no space, no camel case, no `K`.
  `karu` takes the text colour and `code` takes the accent, which is the one
  piece of colour in the logo.
* **Code, paths, CLI output: JetBrains Mono** (fallback `SFMono-Regular`,
  `Consolas`).
* Scale: 13px base in the UI (what the mock uses) at 1.5 line height; headings
  18/24/32. Don't ship a new size between 13 and 18 — the UI reads as one texture.

## Voice

The README's first line is the register to match: factual, lowercase-friendly,
no exclamation marks, no "revolutionary", no emoji in product copy.

* Say what the thing does and what it refuses to do. "Shell commands are
  explicitly **not** sandboxed" earns more trust than a feature list.
* Prefer the concrete number over the adjective: "one binary, ~7ms startup"
  rather than "blazing fast".
* Second person, present tense: "you stop it", "the server embeds the UI".
* Sentence case for headings and buttons.

Tagline (banner): **A coding agent that is one binary, not a stack.**
Facts line (social card): **one Rust binary · SQLite · any OpenAI-compatible endpoint.**

## Assets not to use

`directions/` holds three rejected directions (Prompt, Seal, Loop). They stay in
the repo as the record of the decision — do not ship them. The E mark, the
vermilion stamp and the teal ring are not part of the identity.
