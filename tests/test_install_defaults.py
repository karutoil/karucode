#!/usr/bin/env python3
"""Tests for how packaging/install.sh picks the account the service runs as.

The installer's job here is narrow and easy to get wrong: install the binary and
the unit, and run them **as the human who invoked sudo** - not as root, not as a
dedicated `karucode` system account, and not as whatever `$USER` happens to say
inside a sudo session. Everything this script decides hangs off that one
resolution, so this file tests it directly rather than inferring it from a full
install.

Everything below is a real `sh packaging/install.sh` process. It runs with
`--dry-run`, which the installer supports without root and which mutates nothing,
against a real binary and a `--prefix` in a temp directory. The `--dry-run` output
is not a description of the plan: install.sh prints the drop-in it *would* write,
so `User=`, `Group=` and `KARUCODE_DATA_DIR=` are asserted from the actual bytes.

The host needs: `sh`, `getent`, and a few accounts to name (`root`, and at least
one non-root account). Cases that are impossible to set up portably - an account
whose primary group is not named after it - are skipped with a reason rather than
asserted vacuously.

    python3 tests/test_install_defaults.py          # all cases
    python3 tests/test_install_defaults.py -v       # one line per case

What is covered:

* a sudo run resolves to `$SUDO_USER`, and never to `root`/uid 0;
* a plain run resolves to `$USER`, and `$LOGNAME` is used when `$USER` is root;
* a run with no usable environment falls back to the system account **loudly**;
* `--service-user` rejects root in every spelling: `root`, `0`, `00`, `root:root`,
  `0:0`, and refuses an account that does not exist;
* the group comes from the account's real gid, and `--service-group` overrides it;
* the data directory is `~/.local/share/karucode` for a person, but the account's
  own home for a system account (not `<home>/.local/share/karucode`);
* the service account is printed in the summary, so it can be checked against
  `systemctl show -p User`;
* `--no-systemd` installs no unit and resolves no account at all.
"""

from __future__ import annotations

import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
INSTALL_SH = ROOT / "packaging" / "install.sh"
UNIT_FILE = ROOT / "packaging" / "karucode.service"

# A real, runnable "karucode binary". The installer smoke-tests `--version` before
# it will install anything, and reports this string in its summary.
FAKE_BINARY = """\
#!/bin/sh
echo "karucode 9.9.9-test"
"""


def have_account(name):
    return subprocess.run(["getent", "passwd", name], capture_output=True).returncode == 0


def account_field(name, field):
    out = subprocess.run(["getent", "passwd", name], capture_output=True, text=True).stdout
    if not out:
        return ""
    return out.strip().split(":")[field]


def group_name(gid):
    out = subprocess.run(["getent", "group", gid], capture_output=True, text=True).stdout
    return out.strip().split(":")[0] if out else ""


def login_uid_now():
    """The session login uid, with the kernel's 'unset' sentinel mapped to ''."""
    try:
        value = pathlib.Path("/proc/self/loginuid").read_text().strip()
    except OSError:
        return ""
    return "" if value in ("", "4294967295") else value


class Case(unittest.TestCase):
    """A temp prefix and binary, plus a helper that runs the installer dry."""

    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="kc-install-defaults-"))
        self.addCleanup(shutil.rmtree, self.tmp, True)

        self.binary = self.tmp / "karucode"
        self.binary.write_text(FAKE_BINARY, encoding="utf-8")
        self.binary.chmod(0o755)

        self.prefix = self.tmp / "prefix"
        self.prefix.mkdir()
        self.workspace = self.tmp / "ws"
        self.workspace.mkdir()

        # The account that runs this test process: a stand-in for "the human".
        import pwd

        self.me = pwd.getpwuid(os.getuid()).pw_name

    # -- running the installer --------------------------------------------

    def run_install(self, env=None, args=(), systemd=True):
        """Run install.sh --dry-run with a controlled environment.

        `env` replaces the whole environment apart from PATH, so a case can say
        exactly which of SUDO_USER/LOGNAME/USER exist. HOME is a temp directory
        unless a case overrides it.
        """
        base = {
            "PATH": os.environ.get("PATH", "/usr/local/bin:/usr/bin:/bin"),
            "HOME": str(self.tmp / "home"),
            # Keep the unit and drop-in inside this test's temp dir. Without it the
            # dry run reads the host's real /etc/systemd/system/<name>.service.d
            # and *carries that install's* data dir/host/port/user into the output,
            # so a case would assert against this machine's configuration instead
            # of the flags it passed - and the result would differ per host.
            "KARUCODE_UNIT_DIR": str(self.tmp / "systemd"),
        }
        (self.tmp / "home").mkdir(exist_ok=True)
        (self.tmp / "systemd").mkdir(exist_ok=True)
        base.update(env or {})
        argv = [
            "sh",
            str(INSTALL_SH),
            "--dry-run",
            "--binary",
            str(self.binary),
            "--prefix",
            str(self.prefix),
            "--workspace",
            str(self.workspace),
            # Use the shipped unit as-is, by path. Otherwise the installer looks next
            # to the binary (or fetches it), and a dry run reports what it found
            # there rather than what this checkout ships.
            "--unit-file",
            str(UNIT_FILE),
        ]
        if systemd:
            # The dry run never touches systemd, but without this the script would
            # fall back to --no-systemd in a container and skip the very code
            # under test.
            argv.append("--force-systemd")
        argv.extend(args)
        return subprocess.run(argv, capture_output=True, text=True, env=base, timeout=120)

    def dropin(self, result):
        """The drop-in the dry run printed, as a dict of its [Service] keys."""
        lines = result.stdout.splitlines()
        try:
            start = next(i for i, line in enumerate(lines) if "would write" in line and "10-install.conf" in line)
        except StopIteration:  # pragma: no cover - only on a broken installer
            self.fail(f"the dry run did not print the drop-in it would write:\n{result.stdout}\n{result.stderr}")
        values = {}
        for line in lines[start + 1 :]:
            if not line.startswith("    | "):
                break
            body = line[len("    | ") :]
            if "=" in body and not body.startswith("#"):
                key, _, value = body.partition("=")
                values.setdefault(key, value)
        return values

    def summary_account(self, result):
        match = re.search(r"^Service account: (\S+?):(\S+?)(?: |$)", result.stdout, re.MULTILINE)
        if not match:
            self.fail(f"no 'Service account:' line in the summary:\n{result.stdout}\n{result.stderr}")
        return match.group(1), match.group(2)

    def assert_ok(self, result):
        self.assertEqual(result.returncode, 0, f"exit {result.returncode}\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}")

    # -- assertions --------------------------------------------------------

    def assert_service_account(self, result, user, group=None):
        self.assert_ok(result)
        values = self.dropin(result)
        self.assertEqual(values.get("User"), user, f"drop-in User= is wrong\n{result.stdout}")
        if group is not None:
            self.assertEqual(values.get("Group"), group, f"drop-in Group= is wrong\n{result.stdout}")
        self.assertEqual(
            self.summary_account(result)[0],
            user,
            f"the summary and the drop-in disagree\n{result.stdout}",
        )


# --- who is the service? ---------------------------------------------------


class ResolveTheHuman(Case):
    """`sudo sh install.sh` must run the service as the human, not root."""

    def test_sudo_resolves_to_sudo_user(self):
        result = self.run_install({"SUDO_USER": self.me, "USER": "root", "LOGNAME": self.me})
        self.assert_service_account(result, self.me)

    def test_sudo_never_yields_root_just_because_user_is_root(self):
        # The original bug: $USER is root under sudo, so a fallback to $USER made
        # the service account root and then fell through to the system account.
        result = self.run_install({"SUDO_USER": self.me, "USER": "root"})
        self.assert_service_account(result, self.me)
        self.assertNotEqual(self.dropin(result).get("User"), "root")

    def test_plain_run_resolves_to_the_invoking_user(self):
        result = self.run_install({"USER": self.me, "LOGNAME": self.me})
        self.assert_service_account(result, self.me)

    def test_logname_is_used_when_user_is_root(self):
        # sudo resets $USER to root but leaves $LOGNAME alone; a `sudo -s` shell
        # can lose $SUDO_USER. LOGNAME is the next-best evidence of the human.
        result = self.run_install({"USER": "root", "LOGNAME": self.me})
        self.assert_service_account(result, self.me)

    def test_sudo_user_of_root_falls_through_to_the_human(self):
        # `sudo` from a root shell sets SUDO_USER=root. That is not the answer, and
        # it must not be accepted as one - the next candidate wins instead.
        result = self.run_install({"SUDO_USER": "root", "USER": "root", "LOGNAME": self.me})
        self.assert_service_account(result, self.me)

    def test_a_numeric_sudo_user_is_not_treated_as_an_account(self):
        # `SUDO_USER=0` is root spelled as a uid. It must never become the service
        # account, and it must not be passed through to systemd either.
        result = self.run_install({"SUDO_USER": "0", "USER": "root", "LOGNAME": self.me})
        self.assert_service_account(result, self.me)

    @unittest.skipUnless(have_account("karucode"), "no `karucode` system account on this host to use as a fallback")
    def test_no_human_falls_back_to_the_system_account_loudly(self):
        loginuid = login_uid_now()
        if loginuid not in ("", "0"):
            self.skipTest(f"this session has a real login uid ({loginuid}); it would be the answer instead")

        result = self.run_install({"USER": "root"})
        self.assert_ok(result)
        self.assertEqual(self.dropin(result).get("User"), "karucode")
        # Not silent: an operator has to be able to see that the agent will not be
        # able to reach their home directory.
        self.assertIn("could not identify the human", result.stderr)
        self.assertIn("fallback", result.stdout)

    @unittest.skipUnless(have_account("karucode"), "no `karucode` system account on this host")
    def test_the_system_account_uses_its_own_home_as_the_data_dir(self):
        # The regression: the data dir was always `<home>/.local/share/karucode`, so
        # an account whose home is /var/lib/karucode got the nonsensical
        # /var/lib/karucode/.local/share/karucode.
        home = account_field("karucode", 5)
        self.assertEqual(home, "/var/lib/karucode", "this host's karucode home is unusual; the case is moot")
        result = self.run_install({"USER": "root", "SUDO_USER": self.me}, args=["--service-user", "karucode"])
        self.assert_ok(result)
        self.assertNotIn("/var/lib/karucode/.local/share", result.stdout)
        # The unit's own StateDirectory=karucode already creates and owns exactly
        # this path, so the drop-in leaves it alone and sets no data dir of its own.
        values = self.dropin(result)
        self.assertEqual(values.get("User"), "karucode")
        self.assertNotIn("StateDirectory", values)
        self.assertNotIn("Environment", values)
        summary = re.search(r"^Data directory:\s+(\S+)$", result.stdout, re.MULTILINE)
        self.assertIsNotNone(summary, result.stdout)
        self.assertEqual(summary.group(1), home)


# --- root is refused in every spelling --------------------------------------


class RefuseRoot(Case):
    """The agent executes arbitrary commands, so root is never an option."""

    def assert_refused(self, value):
        result = self.run_install(
            {"USER": "root", "SUDO_USER": self.me}, args=["--service-user", value]
        )
        self.assertEqual(result.returncode, 1, f"--service-user {value!r} was accepted:\n{result.stdout}")
        self.assertIn("must not run as root", result.stderr)

    def test_root_name(self):
        self.assert_refused("root")

    def test_uid_zero(self):
        self.assert_refused("0")

    def test_padded_uid_zero(self):
        self.assert_refused("00")

    def test_user_colon_group(self):
        self.assert_refused("root:root")

    def test_uid_colon_gid(self):
        self.assert_refused("0:0")

    def test_a_name_that_does_not_exist_is_refused(self):
        result = self.run_install(
            {"USER": "root", "SUDO_USER": self.me}, args=["--service-user", "no-such-account-kc-test"]
        )
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("no such account", result.stderr)


# --- the group is the account's real gid -----------------------------------


class ResolveTheGroup(Case):
    def test_group_comes_from_the_real_gid(self):
        gid = account_field(self.me, 3)
        expected = group_name(gid)
        self.assertTrue(expected, f"this host has no /etc/group entry for gid {gid}")
        result = self.run_install({"SUDO_USER": self.me, "USER": "root"})
        self.assert_service_account(result, self.me, group=expected)

    def test_an_explicit_group_wins(self):
        result = self.run_install(
            {"SUDO_USER": self.me, "USER": "root"}, args=["--service-group", "nogroup"]
        )
        if not subprocess.run(["getent", "group", "nogroup"], capture_output=True).returncode == 0:
            self.skipTest("this host has no `nogroup` group")
        self.assert_service_account(result, self.me, group="nogroup")

    def test_an_unknown_group_is_refused(self):
        result = self.run_install(
            {"SUDO_USER": self.me, "USER": "root"}, args=["--service-group", "no-such-group-kc-test"]
        )
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("no such group", result.stderr)


# --- the data directory follows the kind of account -------------------------


class DataDirectory(Case):
    def test_a_person_gets_local_share(self):
        result = self.run_install({"SUDO_USER": self.me, "USER": "root"})
        self.assert_ok(result)
        home = pathlib.Path(account_field(self.me, 5))
        self.assertEqual(
            self.dropin(result).get("Environment"),
            f"KARUCODE_DATA_DIR={home}/.local/share/karucode",
        )
        # A directory under $HOME is not the unit's StateDirectory=, so the drop-in
        # has to clear it and set the path itself.
        self.assertIn("StateDirectory", self.dropin(result))

    def test_an_explicit_data_dir_wins(self):
        elsewhere = self.tmp / "data"
        result = self.run_install(
            {"SUDO_USER": self.me, "USER": "root"}, args=["--data-dir", str(elsewhere)]
        )
        self.assert_ok(result)
        self.assertEqual(self.dropin(result).get("Environment"), f"KARUCODE_DATA_DIR={elsewhere}")

    def write_previous_dropin(self, user, data_dir, port="9000"):
        """Pretend an earlier run already installed with these values."""
        dropin_dir = self.tmp / "systemd" / "karucode.service.d"
        dropin_dir.mkdir(parents=True, exist_ok=True)
        (dropin_dir / "10-install.conf").write_text(
            "[Service]\n"
            f"User={user}\n"
            f"Group={user}\n"
            "WorkingDirectory=/srv/app\n"
            f"Environment=KARUCODE_DATA_DIR={data_dir}\n"
            f"Environment=KARUCODE_PORT={port}\n",
            encoding="utf-8",
        )

    @unittest.skipUnless(have_account("karucode"), "no `karucode` system account on this host")
    def test_an_upgrade_keeps_the_data_dir_and_port_of_the_previous_install(self):
        # The regression this covers: the drop-in is rewritten from this run's flags
        # only, so `sudo sh install.sh --version X` (the documented upgrade, no
        # flags) silently repointed the service at the default data dir - a healthy
        # service on an EMPTY database, with the sessions and the provider API key
        # still in the old one.
        self.write_previous_dropin(self.me, "/srv/kc-data")
        result = self.run_install({"SUDO_USER": self.me, "USER": "root"})
        self.assert_ok(result)
        values = self.dropin(result)
        self.assertEqual(values.get("Environment"), "KARUCODE_DATA_DIR=/srv/kc-data")
        self.assertIn("KARUCODE_PORT=9000", result.stdout)

    def test_an_explicit_flag_still_beats_the_previous_install(self):
        self.write_previous_dropin(self.me, "/srv/kc-data")
        result = self.run_install(
            {"SUDO_USER": self.me, "USER": "root"}, args=["--data-dir", "/new/data"]
        )
        self.assert_ok(result)
        self.assertEqual(self.dropin(result).get("Environment"), "KARUCODE_DATA_DIR=/new/data")

    @unittest.skipUnless(have_account("karucode"), "no `karucode` system account on this host")
    def test_changing_the_service_user_does_not_inherit_the_old_accounts_data_dir(self):
        # A carried data dir only makes sense for the account that owned it. Moving
        # the service to another account must recompute from *that* account's home,
        # or the new account cannot write its own database.
        self.write_previous_dropin(self.me, "/srv/kc-data")
        result = self.run_install(
            {"SUDO_USER": self.me, "USER": "root"}, args=["--service-user", "karucode"]
        )
        self.assert_ok(result)
        self.assertNotIn("/srv/kc-data", result.stdout)
        self.assertEqual(self.dropin(result).get("User"), "karucode")


# --- binary-only installs resolve nothing -----------------------------------


class NoSystemd(Case):
    def test_no_account_is_resolved_or_needed(self):
        # No SUDO_USER, no LOGNAME, root $USER: there is no human *and* no unit, so
        # nothing should be resolved, refused or created - an image build has to be
        # able to install just the binary.
        result = self.run_install({"USER": "root"}, args=["--no-systemd"], systemd=False)
        self.assert_ok(result)
        self.assertNotIn("Service account:", result.stdout)
        self.assertNotIn("10-install.conf", result.stdout)
        self.assertIn("no systemd", result.stdout.lower())
        # And root is not refused here, because there is no service account at all.
        self.assertNotIn("must not run as root", result.stderr)

    def test_the_binary_lands_in_prefix(self):
        # --dry-run prints the install(1) it would run instead of running it, so the
        # preview is asserted; that is the whole point of the dry run.
        result = self.run_install({"USER": self.me}, args=["--no-systemd"], systemd=False)
        self.assert_ok(result)
        self.assertIn(f"install -m 0755 {self.binary} {self.prefix}/bin/karucode", result.stdout)


# --- the summary is checkable ----------------------------------------------


class Summary(Case):
    def test_the_summary_names_the_account_and_the_data_dir(self):
        result = self.run_install({"SUDO_USER": self.me, "USER": "root"})
        self.assert_ok(result)
        user, group = self.summary_account(result)
        self.assertEqual(user, self.me)
        self.assertEqual(group, group_name(account_field(self.me, 3)))
        self.assertIn("Data directory:", result.stdout)
        self.assertIn("who ran this installer", result.stdout)

    def test_an_explicit_override_is_not_labelled_as_the_installer(self):
        result = self.run_install({"USER": "root", "SUDO_USER": self.me}, args=["--service-user", "nobody"])
        if not have_account("nobody"):
            self.skipTest("this host has no `nobody` account")
        self.assert_ok(result)
        self.assertNotIn("who ran this installer", result.stdout)


if __name__ == "__main__":
    unittest.main()
