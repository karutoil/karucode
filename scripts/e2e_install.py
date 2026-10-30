#!/usr/bin/env python3
"""End-to-end test for packaging/install.sh, in a real systemd container.

Nothing on the host is installed, enabled or modified: the whole install runs in
a throwaway podman container that boots systemd as PID 1, and the "reboot" case
is a real `podman restart` - systemd boots again from scratch and the enabled
unit has to come back up on its own. That is the property the installer exists to
guarantee, and the only way to test it is to reboot something.

The release download is served from a local HTTP fixture instead of GitHub, so
the tested path is the real one (tarball + SHA256SUMS + unit fetch) without
depending on a published release.

    python3 scripts/e2e_install.py                 # all cases
    python3 scripts/e2e_install.py --keep          # leave the container running
    python3 scripts/e2e_install.py --rebuild-image # redo the image after apt churn

Requirements: podman, and a built binary (cargo build --release -p kc-server).
Exit code 0 means every case passed.
"""

from __future__ import annotations

import argparse
import functools
import hashlib
import http.server
import os
import shutil
import socket
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import traceback

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
INSTALL_SH = os.path.join(REPO_ROOT, "packaging", "install.sh")
UNIT_FILE = os.path.join(REPO_ROOT, "packaging", "karucode.service")
DEFAULT_BINARY = os.path.join(REPO_ROOT, "target", "release", "karucode")

IMAGE = "localhost/karucode-e2e-systemd:latest"
CONTAINER = "karucode-e2e"
TARGET = "x86_64-unknown-linux-gnu"

# --- small helpers ----------------------------------------------------------


class Fail(Exception):
    """A failed assertion inside a case."""


def check(cond, message):
    if not cond:
        raise Fail(message)


def check_in(needle, haystack, what):
    check(needle in haystack, f"{what}: expected {needle!r} in output:\n{haystack}")


def sh(cmd, **kwargs):
    return subprocess.run(cmd, capture_output=True, text=True, **kwargs)


class Pod:
    """Command runner bound to the test container."""

    def __init__(self, name):
        self.name = name

    def exec(self, command, timeout=300, check_rc=False, env=None):
        prefix = ["podman", "exec"]
        for key, value in (env or {}).items():
            prefix += ["-e", f"{key}={value}"]
        result = sh(prefix + [self.name, "sh", "-lc", command], timeout=timeout)
        if check_rc and result.returncode != 0:
            raise Fail(
                f"command failed ({result.returncode}): {command}\n"
                f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
            )
        return result

    def out(self, command, timeout=300):
        result = self.exec(command, timeout=timeout)
        return result.stdout.strip()

    def cp(self, source, dest):
        sh(["podman", "cp", source, f"{self.name}:{dest}"], check=True)

    def wait_for_systemd(self, timeout=120):
        """Block until systemd finished booting (running or degraded)."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            result = self.exec("systemctl is-system-running", timeout=30)
            state = (result.stdout or result.stderr).strip().splitlines()
            state = state[-1] if state else ""
            if state in ("running", "degraded"):
                return state
            time.sleep(1)
        raise Fail(f"systemd did not finish booting within {timeout}s")

    def wait_for_active(self, unit="karucode", timeout=90):
        deadline = time.time() + timeout
        last = ""
        while time.time() < deadline:
            last = self.out(f"systemctl is-active {unit} || true")
            if last == "active":
                return
            time.sleep(1)
        status = self.out(f"systemctl status {unit} --no-pager -l | head -30 || true")
        journal = self.out(f"journalctl -u {unit} -n 40 --no-pager || true")
        raise Fail(f"{unit} never became active (last state: {last})\n{status}\n{journal}")

    def health(self, port, timeout=30):
        """Wait for /api/health to answer, then return the HTTP code."""
        deadline = time.time() + timeout
        code = "000"
        while time.time() < deadline:
            code = self.out(
                f"curl -s -o /dev/null -m 3 -w '%{{http_code}}' http://127.0.0.1:{port}/api/health || echo 000"
            )
            if code == "200":
                return code
            time.sleep(1)
        return code


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def make_release(root, version, binary):
    """Build <root>/v<version>/karucode-<version>-<target>.tar.gz + SHA256SUMS."""
    name = f"karucode-{version}-{TARGET}"
    out_dir = os.path.join(root, f"v{version}")
    os.makedirs(out_dir, exist_ok=True)
    stage = os.path.join(root, "_stage", name)
    os.makedirs(stage, exist_ok=True)
    shutil.copy2(binary, os.path.join(stage, "karucode"))
    os.chmod(os.path.join(stage, "karucode"), 0o755)
    for extra in ("README.md", "LICENSE", "config.example.toml"):
        path = os.path.join(REPO_ROOT, extra)
        if os.path.exists(path):
            shutil.copy2(path, os.path.join(stage, extra))
    tarball = os.path.join(out_dir, f"{name}.tar.gz")
    with tarfile.open(tarball, "w:gz") as archive:
        archive.add(stage, arcname=name)
    with open(os.path.join(out_dir, "SHA256SUMS"), "w", encoding="utf-8") as handle:
        handle.write(f"{sha256(tarball)}  {name}.tar.gz\n")
    return out_dir


def make_bad_release(root, version, binary):
    """A release whose SHA256SUMS does not match: install.sh must refuse it."""
    out_dir = make_release(root, version, binary)
    name = f"karucode-{version}-{TARGET}.tar.gz"
    with open(os.path.join(out_dir, "SHA256SUMS"), "w", encoding="utf-8") as handle:
        handle.write(f"{'0' * 64}  {name}\n")
    return out_dir


class Feed:
    """A local HTTP server standing in for github.com/<repo>/releases."""

    def __init__(self, root):
        handler = functools.partial(QuietHandler, directory=root)
        self.httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        self.port = self.httpd.server_address[1]
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_exc):
        self.httpd.shutdown()
        self.httpd.server_close()

    def url(self, path=""):
        return f"http://127.0.0.1:{self.port}{path}"


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *_args):
        pass


# --- image ------------------------------------------------------------------


def ensure_image(rebuild=False):
    if not rebuild and sh(["podman", "image", "exists", IMAGE]).returncode == 0:
        return
    containerfile = """\
FROM docker.io/library/debian:13
ENV container=podman
RUN apt-get update \\
 && apt-get install -y --no-install-recommends \\
      systemd systemd-sysv dbus curl ca-certificates \\
      iproute2 procps python3 util-linux coreutils passwd tar \\
 && rm -rf /var/lib/apt/lists/*
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
"""
    with tempfile.TemporaryDirectory() as ctx:
        path = os.path.join(ctx, "Containerfile")
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(containerfile)
        result = sh(["podman", "build", "-t", IMAGE, "-f", path, ctx], timeout=900)
    if result.returncode != 0:
        raise SystemExit(f"image build failed:\n{result.stdout}\n{result.stderr}")


def start_container(port):
    sh(["podman", "rm", "-f", CONTAINER])
    result = sh(
        [
            "podman", "run", "-d",
            "--name", CONTAINER,
            "--systemd=always",
            # The unit uses namespace sandboxing (ProtectHostname, ProtectSystem,
            # PrivateTmp). An unprivileged container has no CAP_SYS_ADMIN, so
            # systemd cannot set those up and the unit dies at status=217/USER.
            # That is a container limitation, not an installer bug, and it is why
            # a container deployment should use the Dockerfile (which does not use
            # these options).
            "--privileged",
            # Host networking keeps the loopback binding identical to a real
            # deployment and lets the container reach the local fixture server.
            # The chosen port is never 8787, so a running instance on the host
            # is not disturbed.
            "--network=host",
            IMAGE,
        ]
    )
    if result.returncode != 0:
        raise SystemExit(f"container start failed:\n{result.stdout}\n{result.stderr}")
    pod = Pod(CONTAINER)
    pod.wait_for_systemd()
    return pod


# --- cases ------------------------------------------------------------------


def case_fresh_install(pod, feed, port):
    """Fetch tarball + unit from the feed and install them for real."""
    pod.cp(INSTALL_SH, "/opt/install.sh")
    result = pod.exec(
        f"VERSION=9.9.9 KARUCODE_RELEASE_BASE_URL={feed.url('/v9.9.9')} "
        f"KARUCODE_UNIT_URL={feed.url('/packaging/karucode.service')} "
        f"sh /opt/install.sh --workspace /srv/kc/ws --port {port}",
        timeout=300,
    )
    check(result.returncode == 0, f"installer exited {result.returncode}:\n{result.stdout}\n{result.stderr}")
    check_in("checksum verified", result.stdout, "installer output")
    check_in("active", result.stdout, "installer output")

    check(pod.out("systemctl is-active karucode") == "active", "unit is not active")
    check(pod.out("systemctl is-enabled karucode") == "enabled", "unit is not enabled at boot")
    check(pod.health(port) == "200", "/api/health did not answer 200")

    unit = pod.out("command cat /etc/systemd/system/karucode.service")
    check("[Unit]" in unit and "StartLimitBurst=5" in unit, "installed unit is not the packaged one")

    dropin = pod.out("command cat /etc/systemd/system/karucode.service.d/10-install.conf")
    check_in("WorkingDirectory=/srv/kc/ws", dropin, "drop-in")
    check_in("ReadWritePaths=/srv/kc/ws", dropin, "drop-in")
    check_in("Environment=KARUCODE_DATA_DIR=/var/lib/karucode", dropin, "drop-in")

    check(pod.out("stat -c '%U %a' /var/lib/karucode") == "karucode 700", "state dir owner/mode wrong")
    check(pod.out("stat -c '%U' /srv/kc/ws") == "karucode", "workspace not owned by the service user")
    check(pod.out("stat -c '%a' /etc/karucode/karucode.env") == "600", "env file is not 0600")
    check(pod.out("test -f /var/lib/karucode/karucode.db && echo yes") == "yes", "no database was created")

    # The service must not be running as root, and the hardening must be on.
    check(pod.out("systemctl show -p User --value karucode") == "karucode", "unit is not running as karucode")
    check(
        pod.out("systemctl show -p NoNewPrivileges --value karucode") == "yes",
        "NoNewPrivileges is not in effect",
    )
    return result.stdout


def case_reboot(pod, feed, port):
    """The real question: does it come back after a reboot?"""
    before = pod.out("systemctl show -p MainPID --value karucode")
    check(before not in ("", "0"), "service had no main pid before the reboot")

    result = sh(["podman", "restart", CONTAINER], timeout=180)
    check(result.returncode == 0, f"podman restart failed:\n{result.stdout}\n{result.stderr}")
    pod.wait_for_systemd(timeout=180)

    # No install, no systemctl start: the enabled unit has to start itself.
    pod.wait_for_active("karucode", timeout=120)
    check(pod.health(port) == "200", "/api/health did not come back after the reboot")
    after = pod.out("systemctl show -p MainPID --value karucode")
    check(after not in ("", "0"), "service has no main pid after the reboot")
    return f"pid {before} -> {after} after reboot"


def case_idempotent_rerun(pod, feed, port):
    """A second run must not break the unit, and must not touch the env file."""
    pod.exec("echo '# operator marker' >> /etc/karucode/karucode.env")
    result = pod.exec(
        f"VERSION=9.9.9 KARUCODE_RELEASE_BASE_URL={feed.url('/v9.9.9')} "
        f"KARUCODE_UNIT_URL={feed.url('/packaging/karucode.service')} "
        f"sh /opt/install.sh --workspace /srv/kc/ws --port {port}",
        timeout=300,
    )
    check(result.returncode == 0, f"second run exited {result.returncode}:\n{result.stdout}\n{result.stderr}")
    check_in("operator marker", pod.out("command cat /etc/karucode/karucode.env"), "env file was rewritten")
    check(pod.out("systemctl is-active karucode") == "active", "unit is not active after a re-run")
    check(pod.out("systemctl is-enabled karucode") == "enabled", "unit lost its enablement")
    check(pod.health(port) == "200", "/api/health did not answer after a re-run")
    check(
        pod.out("test -f /usr/local/bin/karucode.old && echo yes") == "yes",
        "no .old copy of the replaced binary",
    )


def case_upgrade(pod, feed, port):
    """Re-running with a newer release must restart onto the new binary."""
    pid_before = pod.out("systemctl show -p MainPID --value karucode")
    result = pod.exec(
        f"VERSION=9.9.10 KARUCODE_RELEASE_BASE_URL={feed.url('/v9.9.10')} "
        f"KARUCODE_UNIT_URL={feed.url('/packaging/karucode.service')} "
        f"sh /opt/install.sh --workspace /srv/kc/ws --port {port}",
        timeout=300,
    )
    check(result.returncode == 0, f"upgrade exited {result.returncode}:\n{result.stdout}\n{result.stderr}")
    check_in("restarting karucode", result.stdout, "upgrade output")
    pid_after = pod.out("systemctl show -p MainPID --value karucode")
    check(pid_after != pid_before, f"the service was not restarted (pid still {pid_before})")
    check(pod.out("systemctl is-active karucode") == "active", "unit is not active after the upgrade")
    check(pod.health(port) == "200", "/api/health did not answer after the upgrade")
    # The drop-in is rewritten, so the site paths must survive the upgrade.
    dropin = pod.out("command cat /etc/systemd/system/karucode.service.d/10-install.conf")
    check_in("WorkingDirectory=/srv/kc/ws", dropin, "drop-in after upgrade")
    check_in(f"Environment=KARUCODE_PORT={port}", dropin, "drop-in after upgrade")
    check(
        pod.out("test -f /etc/systemd/system/karucode.service.bak && echo yes") == "yes",
        "the previous unit was not backed up",
    )
    return f"pid {pid_before} -> {pid_after}"


def case_bad_checksum(pod, feed, port):
    """A corrupted download must abort and leave the running service alone."""
    before = pod.out("sha256sum /usr/local/bin/karucode | cut -d' ' -f1")
    result = pod.exec(
        f"VERSION=0.0.1 KARUCODE_RELEASE_BASE_URL={feed.url('/v0.0.1')} "
        f"sh /opt/install.sh --workspace /srv/kc/ws --port {port}",
        timeout=300,
    )
    check(result.returncode == 1, f"expected exit 1, got {result.returncode}:\n{result.stdout}\n{result.stderr}")
    check_in("checksum mismatch", result.stderr, "failure message")
    check_in("Nothing was installed", result.stderr, "failure message")
    after = pod.out("sha256sum /usr/local/bin/karucode | cut -d' ' -f1")
    check(before == after, "the installed binary changed after a failed verification")
    check(pod.out("systemctl is-active karucode") == "active", "the failed run disturbed the running service")
    check(pod.health(port) == "200", "/api/health did not answer after the failed run")


def case_dry_run(pod, feed, port):
    """--dry-run must not touch anything."""
    before = pod.out(
        "sha256sum /usr/local/bin/karucode /etc/systemd/system/karucode.service "
        "/etc/systemd/system/karucode.service.d/10-install.conf | cut -d' ' -f1"
    )
    result = pod.exec(
        f"VERSION=9.9.10 KARUCODE_RELEASE_BASE_URL={feed.url('/v9.9.10')} "
        f"sh /opt/install.sh --dry-run --workspace /srv/kc/ws2 --port {port}",
        timeout=120,
    )
    check(result.returncode == 0, f"dry run exited {result.returncode}:\n{result.stdout}\n{result.stderr}")
    check_in("Dry run: nothing was downloaded", result.stdout, "dry-run output")
    after = pod.out(
        "sha256sum /usr/local/bin/karucode /etc/systemd/system/karucode.service "
        "/etc/systemd/system/karucode.service.d/10-install.conf | cut -d' ' -f1"
    )
    check(before == after, "dry run modified an installed file")
    check(pod.out("test -d /srv/kc/ws2 && echo yes") != "yes", "dry run created the workspace")
    check(pod.out("systemctl is-active karucode") == "active", "dry run disturbed the service")


def case_port_conflict(pod, feed, port):
    """A busy port must be reported, not hidden in the journal."""
    pod.exec("systemctl stop karucode; systemctl reset-failed karucode || true")
    # Hold the port so the unit cannot bind it. A plain `&` dies with the exec
    # session, so use a transient unit - and cleanup stops it properly.
    pod.exec(
        f"systemd-run --unit=holdport --quiet /usr/bin/python3 -c "
        f"\"import socket,time; s=socket.socket(); "
        f"s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); "
        f"s.bind(('127.0.0.1', {port})); s.listen(1); time.sleep(300)\"",
        check_rc=True,
    )
    try:
        deadline = time.time() + 30
        held = False
        while time.time() < deadline:
            if pod.out("ss -Hltn 'sport = :%d' | wc -l" % port).strip() == "1":
                held = True
                break
            time.sleep(0.5)
        check(held, "the port holder never bound the port; the case proves nothing")

        result = pod.exec(
            f"VERSION=9.9.10 KARUCODE_RELEASE_BASE_URL={feed.url('/v9.9.10')} "
            f"KARUCODE_UNIT_URL={feed.url('/packaging/karucode.service')} "
            f"sh /opt/install.sh --workspace /srv/kc/ws --port {port}",
            timeout=300,
        )
        check_in("already in use", result.stderr, "preflight warning")
        check(
            result.returncode == 2,
            f"expected exit 2 (installed, not running), got {result.returncode}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}\n"
            f"listeners:\n{pod.out('ss -ltnp | grep -E \"karucode|python\" || true')}",
        )
        check_in("Service: NOT running", result.stdout, "failure summary")
        check_in("journalctl -u karucode", result.stdout, "failure summary")
    finally:
        pod.exec("systemctl stop holdport 2>/dev/null || true")
        # Recover: the port is free again, so the unit must start. reset-failed is
        # required because Restart=on-failure + StartLimitBurst left it in the
        # start-limit-hit state (which is exactly what install.sh clears on its own).
        pod.exec("systemctl reset-failed karucode 2>/dev/null || true")
        pod.exec("systemctl start karucode 2>/dev/null || true")
        pod.wait_for_active("karucode", timeout=60)


def case_no_systemd(pod, feed):
    """The binary-only path must work without root and without a unit."""
    result = pod.exec(
        "sh /opt/install.sh --no-systemd --prefix /opt/kc --binary /usr/local/bin/karucode",
        timeout=120,
    )
    check(result.returncode == 0, f"--no-systemd exited {result.returncode}:\n{result.stdout}\n{result.stderr}")
    check(pod.out("test -x /opt/kc/bin/karucode && echo yes") == "yes", "binary was not installed to --prefix")
    check(pod.out("/opt/kc/bin/karucode --version"), "the installed binary does not run")


def case_allow_jit(pod, feed, port):
    """--allow-jit must show up in the drop-in."""
    result = pod.exec(
        f"VERSION=9.9.10 KARUCODE_RELEASE_BASE_URL={feed.url('/v9.9.10')} "
        f"KARUCODE_UNIT_URL={feed.url('/packaging/karucode.service')} "
        f"sh /opt/install.sh --workspace /srv/kc/ws --port {port} --allow-jit",
        timeout=300,
    )
    check(result.returncode == 0, f"--allow-jit exited {result.returncode}:\n{result.stdout}\n{result.stderr}")
    dropin = pod.out("command cat /etc/systemd/system/karucode.service.d/10-install.conf")
    check_in("MemoryDenyWriteExecute=no", dropin, "drop-in")
    check(
        pod.out("systemctl show -p MemoryDenyWriteExecute --value karucode") == "no",
        "MemoryDenyWriteExecute is still on",
    )


def case_existing_user_home(pod, feed):
    """Running as an existing account, with the data dir under $HOME.

    This is the migration path for someone who already ran karucode by hand: keep
    the account and the data, let systemd own the lifecycle. It is also the case
    that exercises ProtectHome=read-only with a nested ReadWritePaths=.
    """
    port = free_port()
    pod.exec(
        "useradd --create-home --shell /bin/bash kcapp && "
        "install -d -m 0700 -o kcapp -g kcapp /home/kcapp/.local/share/karucode && "
        "install -d -m 0750 -o kcapp -g kcapp /home/kcapp/ws",
        check_rc=True,
    )
    result = pod.exec(
        f"VERSION=9.9.10 KARUCODE_RELEASE_BASE_URL={feed.url('/v9.9.10')} "
        f"KARUCODE_UNIT_URL={feed.url('/packaging/karucode.service')} "
        f"sh /opt/install.sh --service-user kcapp "
        f"--data-dir /home/kcapp/.local/share/karucode "
        f"--workspace /home/kcapp/ws --port {port}",
        timeout=300,
    )
    check(result.returncode == 0, f"installer exited {result.returncode}:\n{result.stdout}\n{result.stderr}")
    check(pod.out("systemctl show -p User --value karucode") == "kcapp", "unit is not running as the requested user")
    check(pod.health(port) == "200", "/api/health did not answer as the existing user")
    rw = pod.out("systemctl show -p ReadWritePaths --value karucode")
    check("/home/kcapp/ws" in rw, f"workspace missing from ReadWritePaths: {rw!r}")
    check(
        "/home/kcapp/.local/share/karucode" in rw,
        f"custom data dir missing from ReadWritePaths: {rw!r}",
    )
    check(
        pod.out("stat -c '%U' /home/kcapp/.local/share/karucode") == "kcapp",
        "the existing data dir changed owner",
    )
    check(
        pod.out("test -f /home/kcapp/.local/share/karucode/karucode.db && echo yes") == "yes",
        "the database was not created in the existing data dir",
    )
    return "service runs as an existing user with its data under $HOME"


# --- driver -----------------------------------------------------------------


CASES = [
    ("fresh install from a fetched release", case_fresh_install),
    ("reboot brings the service back", case_reboot),
    ("re-running the installer is safe", case_idempotent_rerun),
    ("upgrade restarts onto the new binary", case_upgrade),
    ("a bad checksum aborts cleanly", case_bad_checksum),
    ("--dry-run changes nothing", case_dry_run),
    ("a busy port is reported clearly", case_port_conflict),
    ("--no-systemd installs only the binary", case_no_systemd),
    ("--allow-jit relaxes W^X in the drop-in", case_allow_jit),
    ("an existing user with data under $HOME works", case_existing_user_home),
]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", default=DEFAULT_BINARY, help="karucode binary to package (default: target/release)")
    parser.add_argument("--keep", action="store_true", help="leave the container and fixtures for inspection")
    parser.add_argument("--rebuild-image", action="store_true", help="rebuild the systemd test image")
    args = parser.parse_args(argv)

    if not os.path.exists(args.binary):
        raise SystemExit(
            f"no binary at {args.binary}\nbuild one first: cargo build --release -p kc-server"
        )
    if sh(["podman", "--version"]).returncode != 0:
        raise SystemExit("podman is required for this test")

    work = tempfile.mkdtemp(prefix="kc-e2e-")
    port = free_port()
    failures = 0
    pod = None
    try:
        releases = os.path.join(work, "releases")
        os.makedirs(releases, exist_ok=True)
        make_release(releases, "9.9.9", args.binary)
        make_release(releases, "9.9.10", args.binary)
        # A release whose checksum cannot match.
        make_bad_release(releases, "0.0.1", args.binary)
        # Serve the packaged unit next to the releases, like the repo does.
        os.makedirs(os.path.join(releases, "packaging"), exist_ok=True)
        shutil.copy2(UNIT_FILE, os.path.join(releases, "packaging", "karucode.service"))

        with Feed(releases) as feed:
            ensure_image(args.rebuild_image)
            pod = start_container(port)
            print(f"container {CONTAINER} booted (systemd PID 1), feed on {feed.url()} , service port {port}\n")

            for index, (name, case) in enumerate(CASES, 1):
                started = time.time()
                try:
                    args_for_case = case.__code__.co_argcount
                    detail = case(*([pod, feed, port][:args_for_case]))
                    status = "PASS"
                except Fail as exc:
                    status = "FAIL"
                    failures += 1
                    detail = str(exc)
                except Exception:
                    status = "ERROR"
                    failures += 1
                    detail = traceback.format_exc()
                elapsed = time.time() - started
                print(f"[{status}] {index}. {name} ({elapsed:.1f}s)")
                if detail:
                    for line in str(detail).rstrip().splitlines():
                        print(f"        {line}")
            print()
            if failures:
                print(f"{failures} case(s) failed. Container left in place: podman exec -it {CONTAINER} bash")
            else:
                print("all installer cases passed, including a real reboot")
    finally:
        if pod is not None and not args.keep and not failures:
            sh(["podman", "rm", "-f", CONTAINER])
        if not args.keep and not failures:
            shutil.rmtree(work, ignore_errors=True)
        elif args.keep:
            print(f"kept: container {CONTAINER}, fixtures {work}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())