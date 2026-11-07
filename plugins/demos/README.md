# Demo plugins: what a plugin can do to the interface

Two tiny plugins, each a single Python file, that show what the declarative UI
layer can change. They are demos, not products: read them, copy them, and delete
them when you have seen enough.

| Plugin | What it shows |
| --- | --- |
| [`zen/`](zen/zen.py) | **Chrome surgery.** Hides three built-in controls and contributes a button where one used to be; a live panel (1 s poll) with a clock, a headline number, a chart and a list; one action reachable from a button, the palette and `Ctrl/Cmd+Shift+Z`. |
| [`atlas/`](atlas/atlas.py) | **Two panels and state.** A map tab and a history tab, one click that refreshes both, a snapshot written to `.karucode/atlas/`, and a tool (`atlas_scan`) that reports the same numbers the map draws. |

## Installing them

By config entry:

```toml
[[plugins]]
name = "zen"
command = ["python3", "/absolute/path/to/karucode/plugins/demos/zen/zen.py"]
enabled = true
permissions = ["files.read"]

[[plugins]]
name = "atlas"
command = ["python3", "/absolute/path/to/karucode/plugins/demos/atlas/atlas.py"]
enabled = true
permissions = ["files.read", "files.write"]
```

…or copy the directories into `<KARUCODE_DATA_DIR>/plugins/` (each has a
`plugin.toml`) and press **Settings → Plugins & MCP → Rescan plugins**, or
`POST /api/plugins/reload`. Nothing needs a server restart.

## Where they change the interface

| Where | zen | atlas |
| --- | --- | --- |
| Session header | a **Zen** button | — |
| Results header | — | a **Snapshot** button |
| Command palette | "Zen: how much have I written?" | "Atlas: snapshot…", "Atlas: where does the weight sit?" |
| Keyboard | `Ctrl/Cmd+Shift+Z` | `Ctrl/Cmd+Alt+S` |
| Hides | Agents tab, Context button, composer paperclip | — |
| Inspector tabs | **Zen** (live) | **Atlas**, **Snapshots** |
| Tools for the agent | — | `plugin__atlas__atlas_scan` |

Both plugins' hides are on the app's own allowlist, and **Settings → Plugins &
MCP → Controls in the interface** puts any of them back — or switches a
contributed button or command off — without disabling the plugin.

## How the panels are drawn

A panel returns data, never markup, so the app decides what it looks like. The
app understands a small **`view` vocabulary**, which is what makes these panels
look like the rest of the product instead of a JSON tree:

```json
{"view": "stack", "views": [
  {"view": "clock", "label": "Writing", "remaining_secs": 932, "running": true, "tone": "accent"},
  {"view": "stat", "label": "Words in prose", "value": "12,408", "detail": "across 31 files", "tone": "ok"},
  {"view": "bars", "label": "Heaviest areas", "bars": [{"label": "crates", "value": 812345}, {"label": "web", "value": 220100}]},
  {"view": "list", "title": "Recently touched", "items": [{"text": "docs/PLUGINS.md", "detail": "2 min ago"}]},
  {"view": "progress", "label": "Snapshots taken", "fraction": 0.3, "detail": "3 of 10"}
]}
```

Every field is plain text or a number — no markup, no URL, no class name, no
style — and each is validated and capped, so a payload that does not match keeps
the plain JSON view instead of breaking the tab. Give a panel a `refresh_ms` and
the `clock` really ticks, because each poll is a fresh payload.

`view` is optional. A panel that returns `{"items": […]}` still renders as before:
this vocabulary is additive.
