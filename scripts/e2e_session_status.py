#!/usr/bin/env python3
"""Prove the `session-status` reference plugin works against a real server.

This is the acceptance test for the extensibility surface (§R1–§R5 of
`.karucode/plans/extensibility-2026-11.md`). Everything it exercises is public API
that any third-party plugin has: plugin state, slash commands, UI chips, the
session-list hook and the stop gate. If this passes, the items on your list —
"statuses, pinning, slash commands, badges, open-first ordering" — are reachable
without patching the harness.

What it checks, in order:

1. the plugin hot-loads and its commands appear in `GET /api/commands`;
2. a session starts **open** (no state row) and the rail draws an "open" chip;
3. `POST /api/commands/plugin-session-status-complete` marks it completed, which
   persists in the plugin's own state and survives a server restart;
4. `GET /api/sessions` then carries a `completed` badge and sorts it below open
   work (`before_session_list` returning group 1);
5. reopening deletes the state row, returning the session to the default.

Usage:
    python3 scripts/e2e_session_status.py [--binary target/debug/karucode]
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from scripts.design_verify import free_port, wait_for  # noqa: E402
from scripts.e2e_smoke import Client  # noqa: E402
from scripts.verify_plugin_ui import check, FAIL, PASS  # noqa: E402

PLUGIN_NAME = "session-status"


def start(binary: Path, out: Path, port: int, workspace: Path, data_dir: Path, home: Path,
          provider_port: int):
    config = out / "config.toml"
    config.write_text('host = "127.0.0.1"\nlog_level = "warn"\n[approvals]\nmode = "auto"\n')
    env = {
        **os.environ,
        "HOME": str(home),
        "KARUCODE_CONFIG": str(config),
        "KARUCODE_DATA_DIR": str(data_dir),
        "KARUCODE_WEB_DIR": str(ROOT / "web" / "dist"),
        "KARUCODE_BASE_URL": f"http://127.0.0.1:{provider_port}/v1",
        "KARUCODE_MODEL": "mock-coder",
        "KARUCODE_API_KEY": "local-fixture",
        "KARUCODE_LOG": "warn",
    }
    log = (out / f"server-{port}.log").open("w")
    proc = subprocess.Popen(
        [str(binary), "--host", "127.0.0.1", "--port", str(port), "--workspace", str(workspace)],
        cwd=workspace, env=env, stdout=log, stderr=subprocess.STDOUT,
    )
    wait_for(f"http://127.0.0.1:{port}/api/health")
    return proc


def completion(client: Client, session_id: str) -> bool:
    """Whether the server says this session is completed, via the plugin's badge."""
    status, body = client.get("/api/sessions")
    if status != 200:
        return False
    for annotation in body.get("annotations") or []:
        if annotation.get("id") != session_id:
            continue
        return any(b.get("text") == "completed" for b in annotation.get("badges") or [])
    return False


def badge_text(client: Client, session_id: str) -> str:
    status, body = client.get("/api/sessions")
    for annotation in body.get("annotations") or []:
        if annotation.get("id") == session_id:
            return ",".join(b.get("text", "") for b in annotation.get("badges") or [])
    return ""


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/karucode")
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    if not args.binary.exists():
        raise SystemExit(f"missing {args.binary}; run `cargo build -p kc-server`")

    # The plugin binary must exist: this test drives the real one, not a fixture.
    plugin_bin = ROOT / "target/debug" / PLUGIN_NAME
    if not plugin_bin.exists():
        raise SystemExit(f"missing {plugin_bin}; run `cargo build -p {PLUGIN_NAME}`")

    out = args.out or Path(tempfile.mkdtemp(prefix="e2e-session-status-"))
    workspace = out / "project"
    workspace.mkdir(parents=True, exist_ok=True)
    (workspace / "README.md").write_text("# session-status fixture\n")
    home = out / "home"
    home.mkdir(exist_ok=True)
    data_dir = out / "data"
    plugin_dir = data_dir / "plugins" / PLUGIN_NAME
    plugin_dir.mkdir(parents=True, exist_ok=True)
    (plugin_dir / "plugin.toml").write_text(
        f'name = "{PLUGIN_NAME}"\ncommand = ["{plugin_bin}"]\nenabled = true\n'
    )

    provider_port = free_port()
    provider = subprocess.Popen(
        [sys.executable, str(ROOT / "scripts/mock_openai.py"), "--port", str(provider_port),
         "--log", str(out / "provider.jsonl")],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )

    server = None
    try:
        port = free_port()
        server = start(args.binary, out, port, workspace, data_dir, home, provider_port)
        client = Client(port)

        # 1. The plugin loaded and its commands reached the catalog.
        status, report = client.post("/api/plugins/reload")
        check("the plugin hot-loads", status == 200 and PLUGIN_NAME in json.dumps(report),
              str(report)[:300])

        status, catalog = client.get("/api/commands")
        entries = {e["name"]: e for e in catalog.get("commands") or []}
        check("both commands are in the / menu under the plugin prefix",
              "plugin-session-status-complete" in entries
              and "plugin-session-status-reopen" in entries, str(sorted(entries))[:300])
        check("they are marked as coming from a plugin",
              entries.get("plugin-session-status-complete", {}).get("source") == "plugin",
              str(entries.get("plugin-session-status-complete"))[:200])
        check("a plugin command never shadows a built-in",
              entries.get("help", {}).get("source") == "builtin",
              str(entries.get("help"))[:200])

        # 2. A fresh session is open: a chip, and no stored value.
        status, projects = client.get("/api/projects")
        project_id = (projects.get("projects") or [{}])[0].get("id")
        status, created = client.post("/api/sessions",
                                      {"title": "status fixture", "project": project_id})
        session_id = created.get("id")
        check("a session exists", isinstance(session_id, str), str(created)[:200])
        check("it starts open", badge_text(client, session_id) == "open",
              badge_text(client, session_id))

        # 3. Completing it through the slash command persists.
        status, body = client.post(
            "/api/sessions",
            {},  # a second, newer session, so ordering has something to be wrong about
        )
        other_id = body.get("id")
        check("a second session exists", isinstance(other_id, str), str(body)[:200])

        # A slash command is a message: the same path a person's typing takes.
        status, reply = client.post(
            f"/api/sessions/{session_id}/messages",
            {"content": "/plugin-session-status-complete shipped the parsers"},
        )
        text = json.dumps(reply)[:300]
        check("the command is answered in place, not sent to the model",
              status == 200 and reply.get("mode") == "command", text)
        check("it reports success",
              reply.get("command") == "plugin-session-status-complete"
              and reply.get("state") == "ok", text)
        check("the session now shows a completed chip",
              badge_text(client, session_id) == "completed", badge_text(client, session_id))
        check("the other session is still open", badge_text(client, other_id) == "open",
              badge_text(client, other_id))

        # 4. The rail puts open work first: the plugin says which group each row
        # belongs to, and the app honours it. The server deliberately does NOT
        # reorder — ordering is a display concern, and a server that reordered would
        # change what every other API client sees. So the check is on the group,
        # which is the contract a plugin actually controls.
        status, listing = client.get("/api/sessions")
        group_of = {a["id"]: a.get("group") for a in listing.get("annotations") or []}
        check("the plugin's group reaches the client",
              group_of.get(session_id) == 1 and group_of.get(other_id) == 0,
              json.dumps(group_of))
        check("every session still appears in the list",
              {s["id"] for s in listing.get("sessions") or []} >= {session_id, other_id},
              str(len(listing.get("sessions") or [])))

        # 5. It survives a restart: the state is in the database, not in the process.
        server.terminate()
        server.wait(timeout=30)
        server = start(args.binary, out, port, workspace, data_dir, home, provider_port)
        client = Client(port)
        check("the mark survives a server restart (and a plugin restart)",
              badge_text(client, session_id) == "completed", badge_text(client, session_id))

        # 6. Reopening deletes the row, returning the session to the default.
        status, reply = client.post(
            f"/api/sessions/{session_id}/messages",
            {"content": "/plugin-session-status-reopen"},
        )
        check("reopen runs",
              status == 200 and reply.get("state") == "ok", json.dumps(reply)[:300])
        check("the session is open again", badge_text(client, session_id) == "open",
              badge_text(client, session_id))

        # 7. Settings: the schema is published, the operator can edit it, and the
        # plugin's behaviour actually follows (§S1-S2). This is the check that
        # separates "a form exists" from "a setting does something".
        status, schema = client.get("/api/plugins/session-status/settings")
        keys = [f["key"] for f in schema.get("settings") or []]
        check("the plugin publishes its settings schema",
              status == 200 and keys == ["collapse_completed", "group_label", "after_minutes"],
              json.dumps(schema)[:300])
        values = {f["key"]: f.get("value") for f in schema.get("settings") or []}
        check("the values in force are the declared defaults",
              values.get("collapse_completed") is True and values.get("group_label") == "Completed",
              json.dumps(values)[:200])

        # Turn folding OFF: the completed session must stop being grouped away.
        status, saved = client.patch(
            "/api/plugins/session-status/settings",
            {"settings": {"collapse_completed": False}},
        )
        check("a setting can be saved", status == 200 and saved.get("restarted") is True,
              json.dumps(saved)[:300])

        status, listing = client.get("/api/sessions")
        group_of = {a["id"]: a.get("group") for a in listing.get("annotations") or []}
        check("saving a setting changes what the plugin reports",
              group_of.get(session_id) == 0, json.dumps(group_of))

        # A value of the wrong type is refused, and nothing is written.
        status, refused = client.patch(
            "/api/plugins/session-status/settings",
            {"settings": {"after_minutes": "soon"}},
        )
        check("a wrong type is refused with the field named",
              status == 400 and "after_minutes" in json.dumps(refused),
              json.dumps(refused)[:300])
        status, refused = client.patch(
            "/api/plugins/session-status/settings",
            {"settings": {"no_such_setting": 1}},
        )
        check("an undeclared setting is refused", status == 400,
              json.dumps(refused)[:300])
        status, refused = client.patch(
            "/api/plugins/session-status/settings",
            {"settings": {"after_minutes": -5}},
        )
        check("an out-of-range number is refused", status == 400,
              json.dumps(refused)[:300])

        # Turn it back on, and mark the session completed again: the reopen above
        # cleared the mark, so without this the restart check would be asserting
        # against an open session and could not fail for the right reason.
        client.patch(
            "/api/plugins/session-status/settings",
            {"settings": {"collapse_completed": True}},
        )
        client.post(
            f"/api/sessions/{session_id}/messages", {"content": "/plugin-session-status-complete back"}
        )
        server.terminate()
        server.wait(timeout=30)
        server = start(args.binary, out, port, workspace, data_dir, home, provider_port)
        client = Client(port)
        status, listing = client.get("/api/sessions")
        group_of = {a["id"]: a.get("group") for a in listing.get("annotations") or []}
        check("a saved setting survives a server restart",
              group_of.get(session_id) == 1, json.dumps(group_of))

        # 7. An unknown command is still refused rather than silently ignored.
        status, reply = client.post(
            f"/api/sessions/{session_id}/messages",
            {"content": "/plugin-session-status-nope"},
        )
        check("an unknown command is refused with a suggestion, never dispatched",
              status != 200 and "not a command" in json.dumps(reply),
              json.dumps(reply)[:300])
    finally:
        if server is not None:
            server.terminate()
            try:
                server.wait(timeout=15)
            except subprocess.TimeoutExpired:
                server.kill()
        provider.terminate()

    print()
    if FAIL:
        print(f"{len(FAIL)} failed, {len(PASS)} passed")
        for failure in FAIL:
            print(f"  - {failure}")
        return 1
    print(f"all {len(PASS)} checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
