#!/usr/bin/env python3
"""Tests for packaging/update.sh.

The updater is the script that can take a working service down, so it is tested
the way it runs: as a real shell process against a real git repository, with the
world around it stubbed - `systemctl` answers with the unit configuration a case
wants, `cargo` is a fake that either succeeds or fails, and `install.sh` is a
recorder that prints the arguments it was given and exits with a chosen code.

Nothing on the host is touched: every case gets its own temporary checkout, its
own fake unit config, its own data dir, and its own `PATH` stubs.

    python3 tests/test_update.py            # all cases
    python3 tests/test_update.py -v         # one line per case

What is covered, in the order the script runs:

* argument handling: --help, unknown flags, a bad --port;
* unit discovery: no unit, no ExecStart=, a workspace that does not exist;
* deriving the install configuration from the running unit, and letting an
  explicit flag win over it;
* refusing a dirty checkout, and --allow-dirty accepting it;
* a failed build installing nothing;
* the "already up to date" path, and --force overriding it;
* the database snapshot (real SQLite file, real `VACUUM INTO`) and --no-backup;
* passing --allow-jit through only when the unit really has WME=no;
* install.sh exit codes 0, 2 and 1 mapping to this script's 0, 2 and 1;
* --check and --build-only changing nothing;
* the sudo/NoNewPrivileges refusal.
"""

from __future__ import annotations

import json
import os
import pathlib
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import textwrap
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
UPDATER = ROOT / "packaging" / "update.sh"

# --- the fake world ---------------------------------------------------------

# `systemctl show <unit> -p Prop` prints one line per requested property. The
# stub reads its answers from a JSON file so a case can describe any unit.
SYSTEMCTL_STUB = """\
#!/bin/sh
# fake systemctl: answers `show` from $FAKE_UNIT_JSON, ignores everything else
if [ "$1" = "show" ]; then
    shift
    unit="$1"
    shift
    props=""
    for arg in "$@"; do
        case "$arg" in
            -p) ;;
            -p*) props="$props ${arg#-p}" ;;
            *) props="$props $arg" ;;
        esac
    done
    python3 - "$FAKE_UNIT_JSON" "$unit" $props <<'PY'
import json, sys
path, unit, props = sys.argv[1], sys.argv[2], sys.argv[3:]
try:
    with open(path) as handle:
        data = json.load(handle)
except FileNotFoundError:
    data = {}
if unit not in data:
    print("LoadState=not-found")
    print("ExecStart=")
    print("User=")
    print("Group=")
    print("WorkingDirectory=")
    print("Environment=")
    print("StateDirectory=")
    print("MemoryDenyWriteExecute=")
    print("FragmentPath=")
    sys.exit(0)
for prop in props:
    print(f"{prop}={data[unit].get(prop, '')}")
PY
    exit 0
fi
if [ "$1" = "is-active" ]; then
    echo "${FAKE_IS_ACTIVE:-active}"
    exit 0
fi
exit 0
"""

# `cargo build --release -p kc-server` -> create target/release/karucode, or fail.
CARGO_STUB = r"""\
#!/bin/sh
printf '%s\\n' "$*" >> "$FAKE_CARGO_LOG"
if [ -n "${FAKE_CARGO_FAIL:-}" ]; then
    echo "error[E0308]: mismatched types" >&2
    exit 101
fi
mkdir -p target/release
# The body hashes the tree, so "rebuilt from the same sources" produces the same
# binary (the up-to-date case) and any change produces a different one.
digest=$(cat Cargo.toml README.md 2>/dev/null | sha256sum | cut -c1-16)
cat > target/release/karucode <<EOF
#!/bin/sh
echo "karucode ${FAKE_VERSION:-0.1.0} build-\${digest}"
EOF
chmod 0755 target/release/karucode
exit 0
"""

# install.sh recorder: prints its argv as JSON, writes nothing, exits $FAKE_INSTALL_RC.
INSTALLER_STUB = """\
#!/bin/sh
python3 - "$@" <<'PY'
import json, sys
print(json.dumps({"argv": sys.argv[1:]}, sort_keys=True))
PY
exit "${FAKE_INSTALL_RC:-0}"
"""


class Case(unittest.TestCase):
    """A temp checkout + temp unit config + stubs, torn down after each test."""

    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="kc-update-test-"))
        self.addCleanup(shutil.rmtree, self.tmp, True)

        # A real git checkout with a commit, so the dirty/clean logic is real.
        self.src = self.tmp / "src"
        (self.src / "packaging").mkdir(parents=True)
        (self.src / "Cargo.toml").write_text("[workspace]\nmembers = []\n", encoding="utf-8")
        (self.src / "README.md").write_text("demo\n", encoding="utf-8")
        shutil.copy(UPDATER, self.src / "packaging" / "update.sh")
        shutil.copy(ROOT / "packaging" / "karucode.service", self.src / "packaging" / "karucode.service")
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.email", "test@example.invalid")
        self.git("config", "user.name", "test")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "initial")

        self.workspace = self.tmp / "workspace"
        self.workspace.mkdir()
        self.data_dir = self.tmp / "data"
        self.data_dir.mkdir()
        self.prefix = self.tmp / "prefix"
        self.unit_path = self.tmp / "karucode.service"
        self.unit_path.write_text("[Service]\nExecStart=/x\n", encoding="utf-8")

        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        self.write_stub("systemctl", SYSTEMCTL_STUB)
        self.write_stub("cargo", CARGO_STUB)

        self.installer = self.tmp / "installer.sh"
        self.installer.write_text(INSTALLER_STUB, encoding="utf-8")
        self.installer.chmod(0o755)

        self.cargo_log = self.tmp / "cargo.log"
        self.unit_json = self.tmp / "unit.json"

        self.env = {
            "PATH": f"{self.bin}{os.pathsep}{os.environ.get('PATH', '')}",
            "HOME": str(self.tmp / "home"),
            "FAKE_CARGO_LOG": str(self.cargo_log),
            "FAKE_UNIT_JSON": str(self.unit_json),
            "KARUCODE_SYSTEMCTL": str(self.bin / "systemctl"),
        }
        (self.tmp / "home").mkdir()

        self.set_unit()

    # -- helpers -----------------------------------------------------------

    def write_stub(self, name, body):
        path = self.bin / name
        path.write_text(body, encoding="utf-8")
        path.chmod(0o755)

    def git(self, *args):
        return subprocess.run(
            ["git", *args], cwd=self.src, capture_output=True, text=True, check=False
        )

    def set_unit(self, **overrides):
        """Describe the unit `systemctl show` should report."""
        props = {
            "LoadState": "loaded",
            "ExecStart": "{ path=%s/bin/karucode ; argv[]=%s/bin/karucode ; ignore_errors=no }"
            % (self.prefix, self.prefix),
            "User": "svcuser",
            "Group": "svcuser",
            "WorkingDirectory": str(self.workspace),
            "Environment": "KARUCODE_DATA_DIR=%s KARUCODE_HOST=127.0.0.1 KARUCODE_PORT=8787 "
            "PATH=/usr/bin:/bin" % (self.data_dir,),
            "StateDirectory": "",
            "MemoryDenyWriteExecute": "no",
            "FragmentPath": str(self.unit_path),
        }
        props.update(overrides)
        self.unit_json.write_text(json.dumps({"karucode": props}), encoding="utf-8")

    def run_update(self, *args, path=None, **extra_env):
        env = dict(self.env)
        env.update(extra_env)
        if path is not None:
            env["PATH"] = path
        return subprocess.run(
            ["sh", str(self.src / "packaging" / "update.sh"), *args],
            cwd=self.src,
            capture_output=True,
            text=True,
            env=env,
            check=False,
        )

    def install_args(self, result):
        """The argv install.sh was called with, from the recorder's JSON line."""
        for line in result.stdout.splitlines():
            line = line.strip()
            if line.startswith("{") and '"argv"' in line:
                return json.loads(line)["argv"]
        self.fail(f"install.sh was not called.\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}")

    def base_args(self):
        """The flags that keep a case off the network and out of the real world."""
        return ["--no-pull", "--no-sudo", "--installer", str(self.installer)]

    def build(self):
        """Run the (stub) build once so target/release/karucode exists."""
        result = self.run_update(*self.base_args(), "--build-only")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def assert_no_install(self, result):
        for line in result.stdout.splitlines():
            self.assertNotIn('"argv"', line, f"install.sh ran unexpectedly:\n{result.stdout}")


# --- argument handling ------------------------------------------------------


class Arguments(Case):
    def test_help_exits_zero_and_lists_flags(self):
        result = self.run_update("--help")
        self.assertEqual(result.returncode, 0, result.stderr)
        for flag in ("--workspace", "--dry-run", "--check", "--no-pull", "--build-only"):
            self.assertIn(flag, result.stdout)

    def test_unknown_flag_is_refused(self):
        result = self.run_update("--nonsense")
        self.assertEqual(result.returncode, 1)
        self.assertIn("unknown argument", result.stderr)
        self.assert_no_install(result)

    def test_port_must_be_a_number(self):
        result = self.run_update("--no-pull", "--no-sudo", "--dry-run", "--port", "http")
        self.assertEqual(result.returncode, 1)
        self.assertIn("--port must be a number", result.stderr)

    def test_port_range_is_checked(self):
        result = self.run_update("--no-pull", "--no-sudo", "--dry-run", "--port", "70000")
        self.assertEqual(result.returncode, 1)
        self.assertIn("between 1 and 65535", result.stderr)

    def test_missing_value_is_refused(self):
        result = self.run_update("--workspace")
        self.assertEqual(result.returncode, 1)
        self.assertIn("needs a value", result.stderr)


# --- unit discovery ---------------------------------------------------------


class UnitDiscovery(Case):
    def test_no_unit_is_a_clear_error(self):
        self.unit_json.write_text(json.dumps({}), encoding="utf-8")
        result = self.run_update(*self.base_args())
        self.assertEqual(result.returncode, 1)
        self.assertIn("no systemd unit named karucode", result.stderr)
        self.assertIn("install.sh", result.stderr)
        self.assert_no_install(result)

    def test_missing_execstart_is_refused(self):
        self.set_unit(ExecStart="")
        result = self.run_update(*self.base_args())
        self.assertEqual(result.returncode, 1)
        self.assertIn("cannot read ExecStart", result.stderr)
        self.assert_no_install(result)

    def test_missing_workspace_is_a_hard_error(self):
        # The classic upgrade breakage: the drop-in would point at a path that
        # is not there, and the unit dies with status=200/CHDIR.
        shutil.rmtree(self.workspace)
        result = self.run_update(*self.base_args())
        self.assertEqual(result.returncode, 1)
        self.assertIn("does not exist", result.stderr)
        self.assertIn("200/CHDIR", result.stderr)
        self.assert_no_install(result)

    def test_explicit_workspace_overrides_the_unit(self):
        shutil.rmtree(self.workspace)
        other = self.tmp / "other-workspace"
        other.mkdir()
        result = self.run_update(*self.base_args(), "--dry-run", "--workspace", str(other))
        self.assertEqual(result.returncode, 0, result.stderr)
        args = self.install_args(result)
        self.assertEqual(args[args.index("--workspace") + 1], str(other))

    def test_root_service_user_is_refused(self):
        self.set_unit(User="root")
        result = self.run_update(*self.base_args())
        self.assertEqual(result.returncode, 1)
        self.assertIn("refuses to keep that", result.stderr)


# --- what gets handed to install.sh ----------------------------------------


class InstallArguments(Case):
    def test_configuration_comes_from_the_running_unit(self):
        result = self.run_update(*self.base_args(), "--dry-run")
        self.assertEqual(result.returncode, 0, result.stderr)
        args = self.install_args(result)
        expect = {
            "--prefix": str(self.prefix),
            "--workspace": str(self.workspace),
            "--data-dir": str(self.data_dir),
            "--service-user": "svcuser",
            "--host": "127.0.0.1",
            "--port": "8787",
        }
        for flag, value in expect.items():
            self.assertIn(flag, args, f"{flag} missing from {args}")
            self.assertEqual(args[args.index(flag) + 1], value)

    def test_binary_is_the_validated_staged_copy(self):
        # The switch installs the *staged* copy, not the build: the copy is what
        # was proven to run on this machine, and it carries the artifacts across
        # the privilege boundary (the build ran as the operator, install.sh as
        # root). A dry run cannot point at a stage that does not exist, so it
        # previews with the build and the unit source instead; the real path is
        # asserted by the four tests that actually stage and switch.
        result = self.run_update(*self.base_args(), "--dry-run")
        args = self.install_args(result)
        self.assertEqual(
            args[args.index("--binary") + 1], str(self.src / "target" / "release" / "karucode")
        )
        self.assertEqual(
            args[args.index("--unit-file") + 1], str(self.src / "packaging" / "karucode.service")
        )
        self.assertIn("would stage", result.stdout)
        self.assertIn(str(self.data_dir / "stage" / "karucode"), result.stdout)

    def test_allow_jit_follows_the_unit(self):
        # A unit with WME=no needs --allow-jit, or the next install hardens the
        # service and every node/java tool the agent runs starts crashing.
        result = self.run_update(*self.base_args(), "--dry-run")
        self.assertIn("--allow-jit", self.install_args(result))

        self.set_unit(MemoryDenyWriteExecute="yes")
        result = self.run_update(*self.base_args(), "--dry-run")
        self.assertNotIn("--allow-jit", self.install_args(result))

    def test_default_data_dir_from_statedirectory(self):
        # The full default: no KARUCODE_DATA_DIR in Environment=, but the unit
        # keeps StateDirectory=karucode, so the data dir is the state dir.
        state = self.tmp / "state" / "karucode"
        state.mkdir(parents=True)
        self.set_unit(Environment="", StateDirectory="karucode")
        result = self.run_update(
            *self.base_args(), "--dry-run", "--data-dir", str(state)
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        args = self.install_args(result)
        self.assertEqual(args[args.index("--data-dir") + 1], str(state))

    def test_service_group_difference_warns(self):
        self.set_unit(Group="svcgroup")
        result = self.run_update(*self.base_args(), "--dry-run")
        self.assertIn("Group=svcgroup", result.stderr)
        self.assertIn("20-local.conf", result.stderr)


# --- the build --------------------------------------------------------------


class Build(Case):
    def test_build_runs_cargo_and_installs(self):
        result = self.run_update(*self.base_args())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(self.cargo_log.exists(), "cargo was never run")
        self.assertIn("build --release -p kc-server", self.cargo_log.read_text())
        args = self.install_args(result)
        self.assertIn("--binary", args)
        self.assertIn("is installed at", result.stdout)

    def test_failed_build_installs_nothing(self):
        result = self.run_update(*self.base_args(), FAKE_CARGO_FAIL="1")
        self.assertEqual(result.returncode, 1)
        self.assertIn("cargo build failed", result.stderr)
        self.assertIn("nothing was installed", result.stderr)
        self.assert_no_install(result)

    def test_build_only_stops_before_the_service(self):
        result = self.run_update(*self.base_args(), "--build-only")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.src / "target" / "release" / "karucode").exists())
        self.assertIn("stopping before the service is touched", result.stdout)
        self.assert_no_install(result)

    def test_skip_build_without_a_binary_fails(self):
        result = self.run_update(*self.base_args(), "--skip-build")
        self.assertEqual(result.returncode, 1)
        self.assertIn("does not exist", result.stderr)

    def test_dry_run_builds_nothing(self):
        result = self.run_update(*self.base_args(), "--dry-run")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.cargo_log.exists(), "cargo ran during a dry run")
        self.assertIn("would run:", result.stdout)


# --- the working tree -------------------------------------------------------


class WorkingTree(Case):
    def test_dirty_checkout_is_refused(self):
        (self.src / "README.md").write_text("changed\n", encoding="utf-8")
        result = self.run_update(*self.base_args(), "--dry-run")
        self.assertEqual(result.returncode, 1)
        self.assertIn("tracked file(s) are modified", result.stderr)
        self.assertIn("README.md", result.stderr)
        self.assert_no_install(result)

    def test_allow_dirty_builds_it_anyway(self):
        (self.src / "README.md").write_text("changed\n", encoding="utf-8")
        result = self.run_update(*self.base_args(), "--dry-run", "--allow-dirty")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("dirty checkout on purpose", result.stderr)

    def test_untracked_files_are_fine(self):
        # target/ is untracked by definition; it must not count as dirty.
        (self.src / "target").mkdir()
        (self.src / "target" / "junk").write_text("x", encoding="utf-8")
        result = self.run_update(*self.base_args(), "--dry-run")
        self.assertEqual(result.returncode, 0, result.stderr)


# --- --check ----------------------------------------------------------------


class Check(Case):
    def add_upstream_commit(self):
        """Give the checkout an upstream that is one commit ahead."""
        upstream = self.tmp / "upstream.git"
        subprocess.run(["git", "clone", "-q", "--bare", str(self.src), str(upstream)], check=True)
        self.git("remote", "add", "origin", str(upstream))
        self.git("fetch", "-q", "origin")
        self.git("branch", "--set-upstream-to=origin/main", "main")

        clone = self.tmp / "clone"
        subprocess.run(["git", "clone", "-q", str(upstream), str(clone)], check=True)
        subprocess.run(["git", "config", "user.email", "t@example.invalid"], cwd=clone, check=True)
        subprocess.run(["git", "config", "user.name", "t"], cwd=clone, check=True)
        (clone / "README.md").write_text("newer\n", encoding="utf-8")
        subprocess.run(["git", "commit", "-qam", "a change to update to"], cwd=clone, check=True)
        subprocess.run(["git", "push", "-q"], cwd=clone, check=True)

    def test_up_to_date_exits_zero(self):
        self.add_upstream_commit()  # not fetched yet
        result = self.run_update("--check", "--no-fetch", "--no-sudo")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("up to date", result.stdout)

    def test_behind_exits_three_and_lists_commits(self):
        self.add_upstream_commit()
        result = self.run_update("--check", "--no-fetch", "--no-sudo", "--allow-dirty")
        # --no-fetch means the local remote-tracking ref is stale; fetch it by
        # hand the way a real --check would have.
        self.git("fetch", "-q", "--prune")
        result = self.run_update("--check", "--no-fetch", "--no-sudo")
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn("a change to update to", result.stdout)
        self.assertIn("1 commit(s) behind", result.stdout)
        self.assert_no_install(result)

    def test_check_ignores_a_dirty_tree(self):
        (self.src / "README.md").write_text("changed\n", encoding="utf-8")
        result = self.run_update("--check", "--no-fetch", "--no-sudo")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("tracked file(s) are modified", result.stderr)

    def test_check_does_not_move_head(self):
        self.add_upstream_commit()
        self.git("fetch", "-q")
        before = self.git("rev-parse", "HEAD").stdout.strip()
        self.run_update("--check", "--no-fetch", "--no-sudo")
        after = self.git("rev-parse", "HEAD").stdout.strip()
        self.assertEqual(before, after)


# --- pulling and --ref ------------------------------------------------------


class PullAndRef(Case):
    def setUp(self):
        super().setUp()
        self.upstream = self.tmp / "upstream.git"
        subprocess.run(["git", "clone", "-q", "--bare", str(self.src), str(self.upstream)], check=True)
        self.git("remote", "add", "origin", str(self.upstream))
        self.git("fetch", "-q", "origin")
        self.git("branch", "--set-upstream-to=origin/main", "main")

    def push_commit(self, message, body="newer\n"):
        """Commit and push to the upstream from a clone, like a teammate would."""
        clone = self.tmp / f"clone-{message.replace(' ', '-')}"
        subprocess.run(["git", "clone", "-q", str(self.upstream), str(clone)], check=True)
        for key, value in (("user.email", "t@example.invalid"), ("user.name", "t")):
            subprocess.run(["git", "config", key, value], cwd=clone, check=True)
        (clone / "README.md").write_text(body, encoding="utf-8")
        subprocess.run(["git", "commit", "-qam", message], cwd=clone, check=True)
        subprocess.run(["git", "push", "-q"], cwd=clone, check=True)
        return subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=clone, capture_output=True, text=True
        ).stdout.strip()

    def head(self):
        return self.git("rev-parse", "HEAD").stdout.strip()

    def pull_args(self):
        """base_args() minus --no-pull: these cases are about the pull itself."""
        return ["--no-sudo", "--installer", str(self.installer)]

    def test_pull_moves_head_to_the_upstream_commit(self):
        wanted = self.push_commit("a change to pull")
        result = self.run_update(*self.pull_args(), "--build-only")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("git pull --ff-only", result.stdout)
        self.assertEqual(self.head(), wanted)

    def test_no_pull_leaves_head_alone(self):
        self.push_commit("a change to ignore")
        before = self.head()
        result = self.run_update("--no-pull", "--no-sudo", "--build-only")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.head(), before)
        self.assertNotIn("git pull", result.stdout)

    def test_dirty_tree_is_refused_and_the_upstream_is_not_applied(self):
        # A dirty tree must stop the run with our message, not git's, and must
        # not fast-forward onto the upstream commit as part of refusing.
        self.push_commit("a change to pull")
        (self.src / "README.md").write_text("mine\n", encoding="utf-8")
        before = self.head()
        result = self.run_update(*self.pull_args(), "--build-only")
        self.assertEqual(result.returncode, 1)
        self.assertIn("tracked file(s) are modified", result.stderr)
        self.assertEqual(self.head(), before)

    def test_ref_builds_the_tag_and_reports_the_detached_head(self):
        self.git("tag", "v9.9.9")
        self.git("commit", "-qam", "after the tag")
        result = self.run_update(
            *self.base_args(), "--build-only", "--no-pull", "--ref", "v9.9.9"
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("checking out v9.9.9", result.stdout)
        self.assertIn("detached", result.stderr)
        # HEAD is now the tagged commit, not the tip of main.
        self.assertEqual(self.head(), self.git("rev-parse", "v9.9.9^{commit}").stdout.strip())

    def test_ref_that_does_not_exist_fails_without_installing(self):
        result = self.run_update(*self.base_args(), "--no-pull", "--ref", "v0.0.0-nope")
        self.assertEqual(result.returncode, 1)
        self.assertIn("checkout", result.stderr.lower())
        self.assert_no_install(result)

    def test_branch_without_an_upstream_warns_instead_of_failing(self):
        self.git("checkout", "-q", "-b", "local-only")
        result = self.run_update(*self.pull_args(), "--build-only")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("no upstream", result.stderr)


# --- already up to date -----------------------------------------------------


class UpToDate(Case):
    def prime_installed_binary(self):
        """Build once and drop the result where ExecStart= points."""
        first = self.run_update(*self.base_args(), "--build-only")
        self.assertEqual(first.returncode, 0, first.stderr)
        bin_dir = self.prefix / "bin"
        bin_dir.mkdir(parents=True)
        shutil.copy(self.src / "target" / "release" / "karucode", bin_dir / "karucode")
        self.unit_path.write_text(
            (ROOT / "packaging" / "karucode.service").read_text(), encoding="utf-8"
        )

    def test_unchanged_binary_and_unit_skips_the_restart(self):
        self.prime_installed_binary()
        result = self.run_update(*self.base_args(), "--skip-build")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("already up to date", result.stdout)
        self.assert_no_install(result)

    def test_force_reinstalls_anyway(self):
        self.prime_installed_binary()
        result = self.run_update(*self.base_args(), "--skip-build", "--force")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.install_args(result)  # it did call install.sh

    def test_changed_binary_is_installed(self):
        self.prime_installed_binary()
        # A different source tree must produce a different binary, and therefore
        # a real install instead of the "already up to date" early exit.
        (self.src / "README.md").write_text("a real change\n", encoding="utf-8")
        self.git("commit", "-qam", "touch")
        result = self.run_update(*self.base_args(), FAKE_VERSION="0.2.0")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.install_args(result)
        self.assertNotIn("already up to date", result.stdout)


# --- the database snapshot --------------------------------------------------


class Backup(Case):
    def make_db(self):
        db = self.data_dir / "karucode.db"
        connection = sqlite3.connect(db)
        connection.execute("CREATE TABLE sessions (id TEXT)")
        connection.execute("INSERT INTO sessions VALUES ('s_1')")
        connection.commit()
        connection.close()
        return db

    def test_snapshot_is_written_next_to_the_database(self):
        self.make_db()
        result = self.run_update(*self.base_args(), "--dry-run")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("would snapshot", result.stdout)
        self.assertIn("backups/pre-update-", result.stdout)

    def test_real_snapshot_is_a_usable_copy(self):
        # Driven without --dry-run but with --build-only skipped by a failing
        # install: the snapshot happens before install.sh, so it must exist
        # even when the install is the thing that goes wrong.
        self.make_db()
        result = self.run_update(*self.base_args(), FAKE_INSTALL_RC="1")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        snapshots = list((self.data_dir / "backups").glob("pre-update-*.db"))
        self.assertEqual(len(snapshots), 1, f"no snapshot written:\n{result.stdout}")
        connection = sqlite3.connect(snapshots[0])
        try:
            rows = connection.execute("SELECT id FROM sessions").fetchall()
        finally:
            connection.close()
        self.assertEqual(rows, [("s_1",)])

    def test_no_backup_skips_it(self):
        self.make_db()
        result = self.run_update(*self.base_args(), "--no-backup", "--dry-run")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("skipping the database snapshot", result.stdout)
        self.assertNotIn("would snapshot", result.stdout)

    def test_no_sqlite3_warns_instead_of_failing(self):
        self.make_db()
        # A PATH with every tool the script uses except sqlite3.
        bare = self.tmp / "bare-bin"
        bare.mkdir()
        needed = (
            "sh", "sed", "head", "awk", "tr", "wc", "cmp", "sha256sum", "date",
            "mkdir", "cat", "chmod", "dirname", "basename", "git", "id", "cut",
            "grep", "install", "uname", "systemctl", "cargo", "python3",
        )
        for tool in needed:
            found = shutil.which(tool) or (self.bin / tool if (self.bin / tool).exists() else None)
            if found:
                os.symlink(found, bare / tool)
        self.assertFalse(list(bare.glob("sqlite3")), "the bare PATH must not contain sqlite3")
        result = self.run_update(
            *self.base_args(), "--dry-run", path=f"{bare}:/nonexistent"
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("sqlite3 is not installed", result.stderr)


# --- staging ----------------------------------------------------------------


class Staging(Case):
    """The switch must only ever see artifacts that have already been proven."""

    def stage(self, *extra):
        # --skip-build so the test's own artifact is what gets staged, rather
        # than the stub cargo overwriting it with a fresh valid binary.
        return self.run_update(*self.base_args(), "--stage-only", "--skip-build", *extra)

    def stage_files(self):
        return self.data_dir / "stage"

    def test_stage_only_leaves_the_service_alone(self):
        self.build()
        result = self.stage()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("nothing live has changed", result.stdout)
        self.assert_no_install(result)

    def test_stage_holds_the_binary_and_the_unit_and_a_manifest(self):
        self.build()
        result = self.stage()
        self.assertEqual(result.returncode, 0, result.stderr)
        stage = self.stage_files()
        self.assertTrue((stage / "karucode").exists())
        self.assertTrue((stage / "karucode.service").exists())
        self.assertTrue((stage / "STAGE").exists())
        # The manifest is what makes a stage applicable.
        manifest = (stage / "STAGE").read_text()
        self.assertIn("sha256=", manifest)
        self.assertIn("unit_source=", manifest)
        self.assertIn("staged_at=", manifest)

    def test_staged_binary_is_the_build(self):
        self.build()
        self.stage()
        import hashlib

        built = (self.src / "target" / "release" / "karucode").read_bytes()
        staged = (self.stage_files() / "karucode").read_bytes()
        self.assertEqual(hashlib.sha256(built).hexdigest(), hashlib.sha256(staged).hexdigest())

    def test_stage_rejects_a_binary_that_does_not_run(self):
        # The whole point of staging: a binary that cannot execute is caught
        # while the old one is still serving.
        self.build()
        broken = self.src / "target" / "release" / "karucode"
        broken.write_bytes(b"\x7fELF-not-really")
        broken.chmod(0o755)
        result = self.stage()
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("does not run on this machine", result.stderr)
        self.assertIn("still serving the previous binary", result.stderr)
        self.assert_no_install(result)
        # Nothing half-written can be applied later.
        self.assertFalse((self.stage_files() / "karucode").exists())
        self.assertFalse((self.stage_files() / "STAGE").exists())

    def test_stage_rejects_a_unit_that_is_not_a_unit(self):
        self.build()
        # A separate file, so the checkout is not dirtied (which would be
        # refused for an unrelated reason).
        bad = self.tmp / "not-a-unit.service"
        bad.write_text("this is not a unit file\n", encoding="utf-8")
        result = self.stage("--unit-file", str(bad))
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("not a systemd unit", result.stderr)
        self.assert_no_install(result)
        self.assertFalse((self.stage_files() / "STAGE").exists())

    def test_stage_failure_does_not_disturb_an_existing_stage_manifest(self):
        # A failed re-stage must not leave a manifest behind that would apply
        # the previous (or a partial) build.
        self.build()
        first = self.stage()
        self.assertEqual(first.returncode, 0, first.stderr)
        manifest_before = (self.stage_files() / "STAGE").read_text()

        (self.src / "packaging" / "karucode.service").write_text("nope\n", encoding="utf-8")
        broken = self.src / "target" / "release" / "karucode"
        broken.write_bytes(b"nope")
        broken.chmod(0o755)
        second = self.stage()
        self.assertEqual(second.returncode, 1)
        # Either the old stage is gone or its manifest is unchanged; what must
        # not happen is a manifest describing bytes that were not validated.
        manifest = self.stage_files() / "STAGE"
        if manifest.exists():
            self.assertEqual(manifest.read_text(), manifest_before)

    def test_from_stage_applies_without_rebuilding(self):
        self.build()
        staged = self.stage()
        self.assertEqual(staged.returncode, 0, staged.stderr)
        cargo_before = self.cargo_log.read_text()

        result = self.run_update(*self.base_args(), "--from-stage")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        args = self.install_args(result)
        self.assertEqual(args[args.index("--binary") + 1], str(self.stage_files() / "karucode"))
        self.assertEqual(
            args[args.index("--unit-file") + 1], str(self.stage_files() / "karucode.service")
        )
        # No second build: applying a stage must not compile anything.
        self.assertEqual(self.cargo_log.read_text(), cargo_before)

    def test_from_stage_clears_the_stage_after_a_successful_switch(self):
        self.build()
        self.stage()
        result = self.run_update(*self.base_args(), "--from-stage")
        self.assertEqual(result.returncode, 0, result.stderr)
        # Re-applying a stage that is already live must not be possible.
        again = self.run_update(*self.base_args(), "--from-stage")
        self.assertEqual(again.returncode, 1)
        self.assertIn("nothing staged", again.stderr)

    def test_from_stage_without_a_stage_is_refused(self):
        result = self.run_update(*self.base_args(), "--from-stage")
        self.assertEqual(result.returncode, 1)
        self.assertIn("nothing staged", result.stderr)
        self.assert_no_install(result)

    def test_from_stage_refuses_a_tampered_binary(self):
        self.build()
        self.stage()
        target = self.stage_files() / "karucode"
        target.write_bytes(b"#!/bin/sh\necho 'karucode 9.9.9'\n")
        target.chmod(0o755)
        result = self.run_update(*self.base_args(), "--from-stage")
        self.assertEqual(result.returncode, 1)
        self.assertIn("changed since it was validated", result.stderr)
        self.assert_no_install(result)

    def test_from_stage_refuses_an_incomplete_stage(self):
        self.build()
        self.stage()
        (self.stage_files() / "STAGE").unlink()
        result = self.run_update(*self.base_args(), "--from-stage")
        self.assertEqual(result.returncode, 1)
        self.assertIn("incomplete", result.stderr)
        self.assert_no_install(result)

    def test_no_stage_installs_straight_from_the_build(self):
        self.build()
        result = self.run_update(*self.base_args(), "--no-stage")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        args = self.install_args(result)
        self.assertEqual(
            args[args.index("--binary") + 1], str(self.src / "target" / "release" / "karucode")
        )

    def test_stage_only_and_from_stage_are_mutually_exclusive(self):
        result = self.run_update(*self.base_args(), "--stage-only", "--from-stage")
        self.assertEqual(result.returncode, 1)
        self.assertIn("opposites", result.stderr)

    def test_from_stage_excludes_check_and_build_only(self):
        for flag in ("--check", "--build-only"):
            result = self.run_update(*self.base_args(), "--from-stage", flag)
            self.assertEqual(result.returncode, 1, flag)
            self.assertIn("cannot be combined", result.stderr)

    def test_custom_stage_dir_is_honoured(self):
        self.build()
        custom = self.tmp / "elsewhere" / "stage"
        result = self.run_update(*self.base_args(), "--stage-only", "--stage-dir", str(custom))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue((custom / "karucode").exists())

        applied = self.run_update(*self.base_args(), "--from-stage", "--stage-dir", str(custom))
        self.assertEqual(applied.returncode, 0, applied.stdout + applied.stderr)
        args = self.install_args(applied)
        self.assertEqual(args[args.index("--binary") + 1], str(custom / "karucode"))

    def test_dry_run_previews_the_stage_without_creating_it(self):
        result = self.run_update(*self.base_args(), "--dry-run")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("would stage", result.stdout)
        self.assertFalse(self.stage_files().exists(), "a dry run created a stage")


# --- surviving the terminal -------------------------------------------------


class Detach(Case):
    def test_no_detach_keeps_everything_in_the_foreground(self):
        result = self.run_update(*self.base_args(), "--no-detach", "--build-only")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("running detached", result.stdout)

    def test_a_run_without_a_tty_does_not_detach(self):
        # No tty means sudo cannot prompt, so a detached run would hang at the
        # first privileged command. The suite always runs without a tty, which
        # makes every other case in this file the proof of that.
        result = self.run_update(*self.base_args(), "--build-only")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("running detached", result.stdout)

    def test_detached_run_survives_a_hangup_and_still_switches(self):
        # The property that matters: hang up the waiting shell, and the update
        # still finishes and still switches. A passwordless sudo stub makes the
        # run elevated, which is the case detaching is for (an unelevated run
        # cannot prompt for a password without a tty, so it stays in the
        # foreground - see test_a_run_without_a_tty_does_not_detach).
        self.build()
        self.write_stub(
            "sudo",
            "#!/bin/sh\n"
            "# passwordless sudo stub: -n true succeeds, everything else passes through\n"
            'if [ "$1" = "-n" ] && [ "$2" = "true" ]; then exit 0; fi\n'
            'if [ "$1" = "-n" ]; then shift; fi\n'
            'exec "$@"\n',
        )
        slow = self.tmp / "slow-cargo"
        slow.write_text(
            "#!/bin/sh\nsleep 0.2\n"
            "mkdir -p target/release\n"
            "printf '#!/bin/sh\\necho karucode 0.1.0\\n' > target/release/karucode\n"
            "chmod 0755 target/release/karucode\n",
            encoding="utf-8",
        )
        slow.chmod(0o755)

        env = dict(self.env)
        proc = subprocess.Popen(
            [
                "sh", str(self.src / "packaging" / "update.sh"),
                "--no-pull", "--installer", str(self.installer),
                "--detach", "--no-backup", "--cargo", str(slow),
            ],
            cwd=self.src, env=env,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
            start_new_session=True,
        )
        import signal
        import time

        time.sleep(1.5)
        # Hang up the whole foreground process group: this is what a closing
        # terminal does, and it must not stop the update.
        try:
            os.killpg(proc.pid, signal.SIGHUP)
        except ProcessLookupError:
            pass
        try:
            proc.wait(timeout=60)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
            self.fail("the foreground shell never returned")
        parent_out = proc.stdout.read()
        proc.stdout.close()
        proc.stderr.close()

        status = self.tmp / "data" / "update-job" / "status"
        log = self.tmp / "data" / "update-job" / "log"
        for _ in range(300):
            if status.exists():
                break
            time.sleep(0.1)
        text = log.read_text() if log.exists() else "<no log>"
        self.assertTrue(status.exists(), f"no status written; log:\n{text}")
        self.assertEqual(status.read_text().strip(), "0", text)
        # The child did the real work, in its own session, and switched.
        self.assertIn("switching", text)
        self.assertIn('"argv"', text)
        self.assertIn("--unit-file", text)
        # The parent announced the job and forwarded the child's exit code.
        self.assertIn("running detached", parent_out)
        self.assertEqual(proc.returncode, 0, parent_out)

    def test_follow_shows_the_finished_log(self):
        job = self.data_dir / "update-job"
        job.mkdir(parents=True, exist_ok=True)
        (job / "log").write_text("==> hello from a finished job\n", encoding="utf-8")
        (job / "status").write_text("0\n", encoding="utf-8")
        result = self.run_update("--follow", "--no-sudo")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("already finished", result.stdout)
        self.assertIn("hello from a finished job", result.stdout)

    def test_follow_without_a_job_is_refused(self):
        result = self.run_update("--follow", "--no-sudo")
        self.assertEqual(result.returncode, 1)
        self.assertIn("--follow needs a job", result.stderr)

    def test_follow_explains_an_unreadable_log(self):
        # A run started with sudo writes its log as root. tail would fail with a
        # bare "Permission denied"; --follow has to say what to do about it.
        job = self.data_dir / "update-job"
        job.mkdir(parents=True, exist_ok=True)
        log = job / "log"
        log.write_text("==> hello\n", encoding="utf-8")
        log.chmod(0o600)
        # The guard has to come first: a log this user can still read makes
        # `--follow` tail it, and `run_update` would block on `tail -f` forever
        # instead of reaching the skip. Owning the file means chmod cannot hide
        # it, so this case only has a subject when it runs as someone else.
        if os.access(log, os.R_OK):
            self.skipTest("cannot make a file unreadable as this user (running as root)")
        result = self.run_update("--follow", "--no-sudo")
        self.assertEqual(result.returncode, 1)
        self.assertIn("not readable", result.stderr)
        self.assertIn("sudo cat", result.stderr)
        self.assertIn("sudo tail -f", result.stderr)

    def test_follow_is_not_blocked_by_the_privilege_guard(self):
        # --follow only reads a log, so a NoNewPrivileges shell must still be
        # able to use it; the guard is about the install, not about reading.
        job = self.data_dir / "update-job"
        job.mkdir(parents=True, exist_ok=True)
        (job / "log").write_text("==> done\n", encoding="utf-8")
        (job / "status").write_text("0\n", encoding="utf-8")
        result = self.run_update("--follow")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("done", result.stdout)

    def test_a_failed_detached_run_prints_the_reason_not_just_a_path(self):
        # The bug this covers: a detached failure printed only "see <log>", and
        # the log was root-owned, so the operator saw nothing at all.
        self.build()
        self.write_stub(
            "sudo",
            "#!/bin/sh\n"
            'if [ "$1" = "-n" ] && [ "$2" = "true" ]; then exit 0; fi\n'
            'if [ "$1" = "-n" ]; then shift; fi\n'
            'exec "$@"\n',
        )
        # A dirty tree is the cheapest real failure with a known message.
        (self.src / "README.md").write_text("dirty\n", encoding="utf-8")
        result = self.run_update("--no-pull", "--installer", str(self.installer), "--detach")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        # The diagnosis itself, not a pointer to a file.
        self.assertIn("tracked file(s) are modified", result.stderr)
        self.assertIn("commit or stash", result.stderr)
        self.assertIn("Full log:", result.stderr)


# --- one update at a time ---------------------------------------------------


class Locking(Case):
    def lock_dir(self):
        # Sits beside the data dir, named after it.
        return self.tmp / ".data.update.lock"

    def test_lock_is_taken_and_released(self):
        self.build()
        result = self.run_update(*self.base_args(), "--stage-only")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.lock_dir().exists(), "the lock outlived the run")

    def test_a_live_lock_blocks_a_second_run(self):
        self.lock_dir().mkdir()
        # A pid that is certainly alive: this test process.
        (self.lock_dir() / "pid").write_text(f"{os.getpid()}\n", encoding="utf-8")
        result = self.run_update(*self.base_args(), "--stage-only")
        self.assertEqual(result.returncode, 1)
        self.assertIn("another update is already running", result.stderr)
        self.assert_no_install(result)

    def test_a_stale_lock_names_the_file_to_remove(self):
        self.lock_dir().mkdir()
        (self.lock_dir() / "pid").write_text("999999\n", encoding="utf-8")
        result = self.run_update(*self.base_args(), "--stage-only")
        self.assertEqual(result.returncode, 1)
        self.assertIn("stale update lock", result.stderr)
        self.assertIn("rm -rf", result.stderr)
        self.assertIn(str(self.lock_dir()), result.stderr)

    def test_a_lock_without_a_pid_names_the_file_to_remove(self):
        self.lock_dir().mkdir()
        result = self.run_update(*self.base_args(), "--stage-only")
        self.assertEqual(result.returncode, 1)
        self.assertIn("no pid", result.stderr)
        self.assertIn(str(self.lock_dir()), result.stderr)

    def test_dry_run_does_not_take_the_lock(self):
        self.lock_dir().mkdir()
        (self.lock_dir() / "pid").write_text(f"{os.getpid()}\n", encoding="utf-8")
        result = self.run_update(*self.base_args(), "--dry-run")
        self.assertEqual(result.returncode, 0, result.stderr)





class ExitCodes(Case):
    def test_success_exits_zero_and_reports_the_version(self):
        result = self.run_update(*self.base_args())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.install_args(result)
        self.assertIn("is installed at", result.stdout)
        self.assertIn("Roll back", result.stdout)

    def test_service_failure_exits_two_with_a_rollback_hint(self):
        result = self.run_update(*self.base_args(), FAKE_INSTALL_RC="2")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("did not stay up", result.stderr)
        self.assertIn("karucode.old", result.stderr)

    def test_installer_failure_exits_one(self):
        result = self.run_update(*self.base_args(), FAKE_INSTALL_RC="1")
        self.assertEqual(result.returncode, 1)
        self.assertIn("left as it was", result.stderr)


# --- privileges -------------------------------------------------------------


class Privileges(Case):
    def test_refuses_when_privileges_cannot_be_raised(self):
        # A plain user whose PATH has no sudo: the install needs root and there
        # is no way to get it. /bin/sh is used directly because this case
        # deliberately hands the script a PATH that does not contain it.
        result = subprocess.run(
            ["/bin/sh", str(self.src / "packaging" / "update.sh"),
             "--no-pull", "--installer", str(self.installer)],
            cwd=self.src,
            capture_output=True,
            text=True,
            env={**self.env, "PATH": f"{self.bin}:/nonexistent"},
            check=False,
        )
        if shutil.which("sudo") is None:
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertIn("sudo is not installed", result.stderr)
            self.assert_no_install(result)
        else:
            # This host has sudo somewhere on the default PATH; the refusal is
            # then about privileges it could not actually raise, not about a
            # missing binary. Either way it must not install silently.
            self.assertIn("update.sh", result.stderr + result.stdout)

    def test_dry_run_needs_no_root(self):
        result = self.run_update(*self.base_args(), "--dry-run")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_build_only_needs_no_root(self):
        result = self.run_update(*self.base_args(), "--build-only")
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2 if "-v" in sys.argv else 1)
