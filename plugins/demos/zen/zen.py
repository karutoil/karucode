#!/usr/bin/env python3
"""`zen` — a demo plugin that turns the workspace into a writing desk.

What it demonstrates, in one install:

  * **Chrome surgery.** It hides three built-in controls (the Agents tab, the
    Context button and the composer's paperclip) and contributes a button where
    the space used to be. The interface really is different while it is installed,
    and Settings → Plugins & MCP → Controls in the interface can put any of it
    back without disabling the plugin.
  * **A live panel.** The `zen` panel asks the host to re-read it every second
    (`refresh_ms: 1000`), so its clock ticks and its word count follows the files
    you just saved. The payload uses the `view` vocabulary (`clock`, `stack`,
    `stat`, `bars`, `list`), so it is drawn by the app's own components instead of
    the generic JSON tree.
  * **One action, three ways in.** The same `ui/action` (`zen`) is reachable from a
    header button, from the command palette, and from `Ctrl/Cmd+Shift+Z`.
  * **A turn for the agent.** Two palette commands post plain user turns, because
    the cheapest useful control is one that needs no handler at all.

It reads the workspace and nothing else: `files.read`, no write, no shell.
"""

from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path

NAME = "zen"
VERSION = "1.0.0"

# Files bigger than this are not counted: a demo must not stall on a blob.
MAX_BYTES = 512 * 1024
# Directories that are never writing. Counting them would make the numbers lie.
SKIP_DIRS = {
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".turbo",
}
# Extensions treated as prose, in the order the chart prefers them.
PROSE = {".md", ".markdown", ".mdx", ".txt", ".rst", ".adoc"}

MANIFEST = {
    "name": NAME,
    "version": VERSION,
    "description": "Writing desk: hides the chrome you do not need, counts your words",
    "protocol": "1",
    "capabilities": ["tools", "panels", "ui"],
    "permissions": ["files.read"],
    "hooks": [],
    "panels": [
        {"id": "zen", "title": "Zen", "icon": "spark", "source": "static", "refresh_ms": 1000}
    ],
    "ui": [
        # The space where the agent list used to be, since this plugin hides it.
        {"kind": "button", "id": "zen", "label": "Zen", "icon": "spark",
         "title": "Words written, and how long you have been at it",
         "slot": "session.header", "action": "zen"},
        {"kind": "keybinding", "id": "zen-key", "label": "Zen: toggle the writing desk",
         "keys": "mod+shift+z", "action": "zen"},
        {"kind": "command", "id": "zen-ask", "label": "Zen: how much have I written?",
         "message": "Count the words in the markdown files of this workspace and tell me "
                    "which file grew the longest."},
        {"kind": "hide", "target": "inspector.tab.agents"},
        {"kind": "hide", "target": "inspector.context"},
        {"kind": "hide", "target": "composer.attach"},
    ],
}

# When the panel was first read, so the clock has something to count from. A real
# plugin would persist this; a demo keeps it in memory, which is enough to show a
# live panel and honest about what it is.
opened_at: float | None = None


def prose_files(workspace: str) -> list[tuple[str, int, float]]:
    """(relative path, words, mtime) for every prose file in the workspace."""
    root = Path(workspace)
    if not root.is_dir():
        return []
    found: list[tuple[str, int, float]] = []
    for path in root.rglob("*"):
        if not path.is_file():
            continue
        parts = set(path.relative_to(root).parts[:-1])
        if parts & SKIP_DIRS:
            continue
        if path.suffix.lower() not in PROSE:
            continue
        try:
            stat = path.stat()
            if stat.st_size > MAX_BYTES:
                continue
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        found.append((str(path.relative_to(root)), len(text.split()), stat.st_mtime))
    found.sort(key=lambda item: item[1], reverse=True)
    return found


def relative_mtime(mtime: float) -> str:
    delta = max(0.0, time.time() - mtime)
    if delta < 90:
        return "just now"
    if delta < 3600:
        return f"{int(delta // 60)} min ago"
    if delta < 86_400:
        return f"{int(delta // 3600)} h ago"
    return f"{int(delta // 86_400)} d ago"


def zen_payload(params: dict) -> dict:
    """The `view` document the inspector draws.

    Deliberately a *stack*: a live clock, one headline number, a chart and a list.
    Every field is plain text or a number — the app decides what it looks like.
    """
    global opened_at
    if opened_at is None:
        opened_at = time.time()
    workspace = params.get("workspace") or os.getcwd()
    files = prose_files(workspace)
    words = sum(entry[1] for entry in files)
    minutes = int((time.time() - opened_at) // 60)

    bars = [
        {"label": Path(name).name[:16], "value": count}
        for name, count, _ in files[:6]
        if count > 0
    ]
    recent = sorted(files, key=lambda entry: entry[2], reverse=True)[:6]

    views: list[dict] = [
        {
            "view": "clock",
            "label": "Writing",
            "remaining_secs": int(time.time() - opened_at),
            "running": True,
            "detail": f"{minutes} min at this desk",
            "tone": "accent",
        },
        {
            "view": "stat",
            "label": "Words in prose",
            "value": f"{words:,}",
            "detail": f"across {len(files)} file{'s' if len(files) != 1 else ''}",
            "tone": "ok" if words else "neutral",
        },
    ]
    if bars:
        views.append({"view": "bars", "label": "Longest files", "bars": bars})
    if recent:
        views.append(
            {
                "view": "list",
                "title": "Touched recently",
                "items": [
                    {"text": name, "detail": relative_mtime(mtime)}
                    for name, _, mtime in recent
                ],
            }
        )
    if not files:
        views.append(
            {
                "view": "list",
                "title": "Nothing to count yet",
                "items": [{"text": "Write a .md file and this panel will find it."}],
            }
        )
    return {"title": "Zen", "data": {"view": "stack", "views": views}}


def send(message: dict) -> None:
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def main() -> int:
    for line in sys.stdin:
        if not line.strip():
            continue
        try:
            request = json.loads(line)
        except json.JSONDecodeError:
            send({"jsonrpc": "2.0", "id": None, "error": {"code": -32700, "message": "bad json"}})
            continue
        method = request.get("method")
        request_id = request.get("id")
        params = request.get("params") or {}

        if method == "initialize":
            send({"jsonrpc": "2.0", "id": request_id, "result": MANIFEST})
        elif method == "tools/list":
            send({"jsonrpc": "2.0", "id": request_id, "result": {"tools": []}})
        elif method == "panels/read":
            send({"jsonrpc": "2.0", "id": request_id, "result": zen_payload(params)})
        elif method == "ui/action":
            files = prose_files(params.get("workspace") or os.getcwd())
            words = sum(entry[1] for entry in files)
            send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "result": {
                        "ok": True,
                        "message": f"Zen: {words:,} words in {len(files)} prose files.",
                        # Re-read the panel so the click updates the numbers too.
                        "refresh": ["zen"],
                    },
                }
            )
        elif method == "shutdown":
            send({"jsonrpc": "2.0", "id": request_id, "result": None})
            return 0
        elif request_id is not None:
            send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {"code": -32601, "message": f"unknown method {method}"},
                }
            )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
