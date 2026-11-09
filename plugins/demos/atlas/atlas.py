#!/usr/bin/env python3
"""`atlas` — a demo plugin that maps the workspace and remembers what it looked like.

Different powers from `zen`, on purpose:

  * **Two panels.** `atlas` and `history` are two tabs from one plugin, and one
    click refreshes both (`refresh: ["atlas", "history"]`).
  * **State of its own.** The **Snapshot** button writes a JSON file under
    `.karucode/atlas/` and the history panel lists what it wrote. The plugin owns
    that state; the app never sees it as anything but data.
  * **A tool the agent can call.** `atlas_scan` returns the same numbers the panel
    draws, so the model and the human look at one implementation.
  * **One action, three ways in.** A header button, a palette command and
    `Ctrl/Cmd+Alt+S` all run `ui/action` `snapshot`.

It writes only inside `.karucode/atlas/`, and only when you press Snapshot.
"""

from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path

NAME = "atlas"
VERSION = "1.0.0"

MAX_FILES = 20_000
# Big enough to matter, small enough not to read a virtual machine image.
MAX_BYTES_SCANNED = 2 * 1024 * 1024
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
    ".karucode",
}

MANIFEST = {
    "name": NAME,
    "version": VERSION,
    "description": "Map the workspace: one panel for the map, one for its history",
    "protocol": "1",
    "capabilities": ["tools", "panels", "ui"],
    "permissions": ["files.read", "files.write"],
    "hooks": [],
    "panels": [
        {"id": "atlas", "title": "Atlas", "icon": "layers", "source": "static", "refresh_ms": 0},
        {"id": "history", "title": "Snapshots", "icon": "list", "source": "static", "refresh_ms": 0},
    ],
    "ui": [
        {"kind": "button", "id": "snapshot", "label": "Snapshot", "icon": "circle-check",
         "title": "Record what the workspace looks like right now",
         "slot": "inspector.header", "action": "snapshot"},
        {"kind": "keybinding", "id": "snapshot-key", "label": "Atlas: take a snapshot",
         "keys": "mod+alt+s", "action": "snapshot"},
        {"kind": "command", "id": "atlas-snapshot", "label": "Atlas: snapshot this workspace",
         "action": "snapshot"},
        {"kind": "command", "id": "atlas-ask", "label": "Atlas: where does the weight sit?",
         "message": "Look at this workspace and tell me which top-level area carries most of "
                    "the code, and what that suggests about where to start reading."},
    ],
}


def skip(relative: Path) -> bool:
    return bool(set(relative.parts[:-1]) & SKIP_DIRS)


def scan(workspace: str) -> list[tuple[str, int]]:
    """(relative path, bytes) for every file worth counting."""
    root = Path(workspace)
    if not root.is_dir():
        return []
    out: list[tuple[str, int]] = []
    for path in root.rglob("*"):
        if len(out) >= MAX_FILES:
            break
        try:
            if not path.is_file():
                continue
            relative = path.relative_to(root)
            if skip(relative):
                continue
            size = path.stat().st_size
        except OSError:
            continue
        out.append((str(relative), size))
    return out


def human(size: int) -> str:
    if size < 1024:
        return f"{size} B"
    if size < 1024 * 1024:
        return f"{size / 1024:.1f} kB"
    return f"{size / (1024 * 1024):.1f} MB"


def areas(files: list[tuple[str, int]]) -> list[tuple[str, int, int]]:
    """(top-level area, bytes, file count), heaviest first."""
    totals: dict[str, list[int]] = {}
    for name, size in files:
        head = name.split(os.sep, 1)[0]
        area = head if os.sep in name else "."
        entry = totals.setdefault(area, [0, 0])
        entry[0] += size
        entry[1] += 1
    return sorted(
        ((area, entry[0], entry[1]) for area, entry in totals.items()),
        key=lambda item: item[1],
        reverse=True,
    )


def storage_dir(workspace: str) -> Path:
    return Path(workspace) / ".karucode" / "atlas"


def snapshots(workspace: str) -> list[tuple[str, int, float]]:
    directory = storage_dir(workspace)
    if not directory.is_dir():
        return []
    out: list[tuple[str, int, float]] = []
    for path in sorted(directory.glob("*.json")):
        try:
            stat = path.stat()
        except OSError:
            continue
        out.append((path.name, stat.st_size, stat.st_mtime))
    return sorted(out, key=lambda item: item[2], reverse=True)


def atlas_payload(params: dict) -> dict:
    workspace = params.get("workspace") or os.getcwd()
    files = scan(workspace)
    total = sum(size for _, size in files)
    ranked = areas(files)
    biggest = sorted(files, key=lambda item: item[1], reverse=True)[:8]
    shots = snapshots(workspace)

    views: list[dict] = [
        {
            "view": "stat",
            "label": "Workspace size",
            "value": human(total),
            "detail": f"{len(files):,} files · {len(ranked)} areas",
            "tone": "neutral",
        }
    ]
    if ranked:
        views.append(
            {
                "view": "bars",
                "label": "Weight by area",
                "bars": [
                    {
                        "label": area,
                        "value": size,
                        # The bar scales from `value`; the human size is what a person reads.
                        "display": human(size),
                        "tone": "accent" if index == 0 else "neutral",
                    }
                    for index, (area, size, _) in enumerate(ranked[:8])
                ],
            }
        )
    views.append(
        {
            "view": "list",
            "title": "Heaviest files",
            "items": [
                {"text": name, "detail": human(size), "status": "neutral"}
                for name, size in biggest
            ]
            or [{"text": "This workspace has no countable files."}],
        }
    )
    views.append(
        {
            "view": "progress",
            "label": "Snapshots taken",
            "fraction": min(1.0, len(shots) / 10.0),
            "detail": f"{len(shots)} of 10",
            "tone": "ok" if shots else "neutral",
        }
    )
    return {"title": "Atlas", "data": {"view": "stack", "views": views}}


def history_payload(params: dict) -> dict:
    workspace = params.get("workspace") or os.getcwd()
    shots = snapshots(workspace)
    if not shots:
        return {
            "title": "Snapshots",
            "data": {
                "view": "list",
                "title": "No snapshots yet",
                "items": [{"text": "Press Snapshot in the results header, or Ctrl/Cmd+Alt+S."}],
            },
        }
    return {
        "title": "Snapshots",
        "data": {
            "view": "stack",
            "views": [
                {
                    "view": "stat",
                    "label": "Snapshots on disk",
                    "value": str(len(shots)),
                    "detail": human(sum(size for _, size, _ in shots)),
                    "tone": "ok",
                },
                {
                    "view": "list",
                    "title": "Most recent first",
                    "items": [
                        {
                            "text": name,
                            "detail": time.strftime("%H:%M:%S", time.localtime(mtime)),
                        }
                        for name, _, mtime in shots[:12]
                    ],
                },
            ],
        },
    }


def take_snapshot(workspace: str) -> tuple[str, dict]:
    """Write one snapshot, and return (file name, what was recorded)."""
    files = scan(workspace)
    ranked = areas(files)
    record = {
        "at": time.time(),
        "workspace": workspace,
        "files": len(files),
        "bytes": sum(size for _, size in files),
        "areas": [{"area": area, "bytes": size, "files": count} for area, size, count in ranked],
        "heaviest": [{"path": name, "bytes": size} for name, size in
                     sorted(files, key=lambda item: item[1], reverse=True)[:20]],
    }
    directory = storage_dir(workspace)
    directory.mkdir(parents=True, exist_ok=True)
    # Milliseconds, not seconds: two snapshots in one second are two snapshots, not
    # one overwritten file.
    name = f"snapshot-{int(record['at'] * 1000)}.json"
    (directory / name).write_text(json.dumps(record, indent=2), encoding="utf-8")
    return name, record


def send(message: dict) -> None:
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def handle_tool(params: dict) -> dict:
    workspace = params.get("arguments", {}).get("workspace") or params.get("workspace") or os.getcwd()
    files = scan(workspace)
    ranked = areas(files)
    summary = ", ".join(
        f"{area} ({human(size)}, {count} file{'s' if count != 1 else ''})"
        for area, size, count in ranked[:6]
    )
    return {
        "ok": True,
        "content": f"{len(files):,} files, {human(sum(size for _, size in files))} total. "
                   f"By area: {summary or 'nothing countable'}.",
        "meta": {"files": len(files), "areas": len(ranked)},
    }


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
            send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "result": {
                        "tools": [
                            {
                                "name": "atlas_scan",
                                "description": "Summarise how the workspace is laid out: file "
                                               "count, total size, and the heaviest areas.",
                                # `parameters`, not `input_schema`: the host reads this field
                                # as the JSON Schema for the arguments object (docs/PLUGINS.md
                                # §4.2). A wrong key is silently an empty schema.
                                "parameters": {
                                    "type": "object",
                                    "properties": {
                                        "workspace": {
                                            "type": "string",
                                            "description": "Absolute path; defaults to the session's.",
                                        }
                                    },
                                },
                            }
                        ]
                    },
                }
            )
        elif method == "tools/call":
            send({"jsonrpc": "2.0", "id": request_id, "result": handle_tool(params)})
        elif method == "panels/read":
            panel = params.get("panel")
            if panel == "atlas":
                send({"jsonrpc": "2.0", "id": request_id, "result": atlas_payload(params)})
            elif panel == "history":
                send({"jsonrpc": "2.0", "id": request_id, "result": history_payload(params)})
            else:
                send({"jsonrpc": "2.0", "id": request_id,
                      "error": {"code": -32602, "message": f"no panel {panel}"}})
        elif method == "ui/action":
            workspace = params.get("workspace") or os.getcwd()
            name, record = take_snapshot(workspace)
            send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "result": {
                        "ok": True,
                        "message": f"Snapshot {name}: {record['files']:,} files, "
                                   f"{human(record['bytes'])}.",
                        # Both tabs are this plugin's, and both changed: one click,
                        # two panels re-read.
                        "refresh": ["atlas", "history"],
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
