#!/usr/bin/env python3
"""Repeatable performance check for karucode.

Measures, on this machine:
  * cold-start time until GET /api/health returns 200
  * idle RSS after 3s with one session open
  * RSS while 3 agent runs stream against the local mock provider
  * release binary size
  * concurrent session/event throughput sanity check

Usage:
    python3 scripts/bench.py [--build] [--runs 3]

Requires python3 (stdlib only). The mock provider is scripts/mock_openai.py.
Results are printed as a markdown table and saved to reports/bench.md.
"""

import argparse
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target" / "release" / "karucode"
REPORT = ROOT / "reports" / "bench.md"


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def wait_health(url: str, timeout: float) -> float:
    start = time.perf_counter()
    deadline = start + timeout
    while time.perf_counter() < deadline:
        try:
            with urllib.request.urlopen(url + "/api/health", timeout=0.5) as r:
                if r.status == 200:
                    return (time.perf_counter() - start) * 1000
        except Exception:
            time.sleep(0.005)
    raise TimeoutError("server did not become healthy")


def rss_kb(pid: int) -> int:
    try:
        with open(f"/proc/{pid}/status", encoding="utf-8") as fh:
            for line in fh:
                if line.startswith("VmRSS:"):
                    return int(line.split()[1])
    except FileNotFoundError:
        pass
    return 0


def api(url: str, method: str = "GET", body=None, timeout=30, headers=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    req.add_header("X-Karucode-Client", "web")
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw = r.read()
            status = r.status
    except urllib.error.HTTPError as e:
        raise RuntimeError(f"{method} {url} -> {e.code}: {e.read()[:400]!r}") from e
    if not raw:
        return {}
    try:
        return json.loads(raw)
    except json.JSONDecodeError as e:
        raise RuntimeError(
            f"{method} {url} -> {status}, non-JSON body: {raw[:400]!r}"
        ) from e


def start_server(workspace: Path, data_dir: Path, port: int, base_url: str, extra_env=None):
    env = os.environ.copy()
    env.update(
        {
            "KARUCODE_HOST": "127.0.0.1",
            "KARUCODE_PORT": str(port),
            "KARUCODE_WORKSPACE": str(workspace),
            "KARUCODE_DATA_DIR": str(data_dir),
            "KARUCODE_BASE_URL": base_url,
            "KARUCODE_MODEL": "mock-coder",
            "KARUCODE_LOG": "warn",
            "KARUCODE_APPROVAL_MODE": "auto",
        }
    )
    if extra_env:
        env.update(extra_env)
    return subprocess.Popen(
        [str(BIN)],
        cwd=str(ROOT),
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )


def start_mock(port: int, log: Path):
    return subprocess.Popen(
        [sys.executable, str(ROOT / "scripts" / "mock_openai.py"), "--port", str(port), "--log", str(log)],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )


def run_bench(runs: int) -> list[dict]:
    results = []
    for i in range(runs):
        with tempfile.TemporaryDirectory(prefix="kc-bench-") as tmp:
            tmp = Path(tmp)
            workspace = tmp / "ws"
            workspace.mkdir()
            (workspace / "README.md").write_text("# bench workspace\n" * 20)
            (workspace / "src").mkdir()
            (workspace / "src" / "main.rs").write_text("fn main() {}\n" * 200)
            subprocess.run(["git", "init", "-q"], cwd=workspace, check=False)

            mock_port = free_port()
            mock = start_mock(mock_port, tmp / "mock.jsonl")
            time.sleep(0.2)
            port = free_port()
            server = start_server(
                workspace, tmp / "data", port, f"http://127.0.0.1:{mock_port}/v1"
            )
            url = f"http://127.0.0.1:{port}"
            try:
                startup_ms = wait_health(url, 30)
                time.sleep(3)
                idle_rss = rss_kb(server.pid)

                sessions = [api(f"{url}/api/sessions", "POST", {"title": f"bench {n}"}) for n in range(3)]
                t0 = time.perf_counter()
                run_ids = []
                for s in sessions:
                    out = api(f"{url}/api/sessions/{s['id']}/messages", "POST", {"content": "[tool:read_file] bench task"})
                    run_ids.append(out["run_id"])

                peak = idle_rss
                deadline = time.time() + 60
                active = True
                while active and time.time() < deadline:
                    peak = max(peak, rss_kb(server.pid))
                    active = False
                    for s in sessions:
                        detail = api(f"{url}/api/sessions/{s['id']}")
                        if any(r["status"] == "running" for r in detail["runs"]):
                            active = True
                    time.sleep(0.05)
                active_ms = (time.perf_counter() - t0) * 1000

                results.append(
                    {
                        "run": i + 1,
                        "startup_ms": round(startup_ms, 1),
                        "idle_rss_mb": round(idle_rss / 1024, 1),
                        "active_peak_rss_mb": round(peak / 1024, 1),
                        "3_runs_ms": round(active_ms, 1),
                        "binary_mb": round(BIN.stat().st_size / (1024 * 1024), 2),
                    }
                )
            finally:
                server.send_signal(signal.SIGINT)
                try:
                    server.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    server.kill()
                mock.terminate()
                mock.wait(timeout=5)
    return results


def counts(workspace: Path) -> dict:
    def lines(cmd):
        try:
            out = subprocess.run(cmd, cwd=workspace, capture_output=True, text=True, timeout=20)
            return sum(1 for _ in out.stdout.splitlines())
        except Exception:
            return -1

    return {
        "rust_lines": lines(["sh", "-c", "find crates -name '*.rs' -print0 | xargs -0 cat | wc -l"]),
        "rust_files": lines(["sh", "-c", "find crates -name '*.rs' | wc -l"]),
        "web_ts_lines": lines(["sh", "-c", "find web/src -name '*.ts' -o -name '*.tsx' | xargs cat 2>/dev/null | wc -l"]),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--build", action="store_true", help="run cargo build --release first")
    parser.add_argument("--runs", type=int, default=3)
    args = parser.parse_args()

    if args.build or not BIN.exists():
        print("building release binary...", flush=True)
        subprocess.run(["cargo", "build", "--release", "-p", "kc-server"], cwd=ROOT, check=True)

    results = run_bench(args.runs)

    uname = subprocess.run(["uname", "-sr"], capture_output=True, text=True).stdout.strip()
    cpu = ""
    try:
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            if line.startswith("model name"):
                cpu = line.split(":", 1)[1].strip()
                break
    except Exception:
        pass
    mem_gb = 0
    try:
        for line in Path("/proc/meminfo").read_text().splitlines():
            if line.startswith("MemTotal"):
                mem_gb = round(int(line.split()[1]) / 1024 / 1024, 1)
                break
    except Exception:
        pass
    rustc = subprocess.run(["rustc", "--version"], capture_output=True, text=True).stdout.strip()

    avg = {
        k: round(sum(r[k] for r in results) / len(results), 1)
        for k in ("startup_ms", "idle_rss_mb", "active_peak_rss_mb", "3_runs_ms", "binary_mb")
    }

    lines = [
        "# karucode performance report",
        "",
        f"Generated: {time.strftime('%Y-%m-%d %H:%M:%S')}",
        f"Host: {uname}, {cpu}, {mem_gb} GiB RAM, {os.cpu_count()} CPUs",
        f"Toolchain: {rustc}",
        f"Binary: target/release/karucode (strip + LTO thin)",
        "",
        "## Method",
        "",
        "`scripts/bench.py` starts the release server with a temp SQLite DB, points it at",
        "`scripts/mock_openai.py` (a local streaming mock, no network), polls `/api/health`",
        "for cold-start time, reads `VmRSS` from `/proc/<pid>/status` after 3s idle, then",
        "starts 3 concurrent agent runs and samples RSS every 50ms until they finish.",
        "",
        "## Results (per run)",
        "",
        "| run | startup ms | idle RSS MiB | peak RSS (3 runs) MiB | 3 runs wall ms | binary MiB |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for r in results:
        lines.append(
            f"| {r['run']} | {r['startup_ms']} | {r['idle_rss_mb']} | {r['active_peak_rss_mb']} | {r['3_runs_ms']} | {r['binary_mb']} |"
        )
    lines += [
        f"| **avg** | **{avg['startup_ms']}** | **{avg['idle_rss_mb']}** | **{avg['active_peak_rss_mb']}** | **{avg['3_runs_ms']}** | **{avg['binary_mb']}** |",
        "",
        "These numbers are specific to the host above and to the mock provider:",
        "RSS during a real run also depends on the provider TLS stack and response sizes.",
    ]

    REPORT.parent.mkdir(exist_ok=True)
    REPORT.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print("\n".join(lines))
    print(f"\nwrote {REPORT}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())