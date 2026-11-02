#!/bin/sh
# shellcheck disable=SC2317  # restore_unit and finish are only called from the EXIT trap
# karucode installer: binary + a systemd unit that actually starts, stays
# enabled across reboots, and survives upgrades.
#
#   curl -fsSLO https://raw.githubusercontent.com/karutoil/karucode/main/packaging/install.sh
#   sudo sh install.sh                          # latest release, /usr/local
#   sudo sh install.sh                          # then add a project in the web UI
#   sudo sh install.sh --workspace /srv/app     # ...or name the first one here
#   sudo sh install.sh --version 0.1.0
#   sh install.sh --prefix "$HOME/.local" --no-systemd   # binary only, no root
#
# Nothing is piped from the network into a shell: the artifact is downloaded to a
# temporary file, its checksum is verified against the release's SHA256SUMS, and
# only then is it unpacked.
#
# Two rules this script is built around:
#
#   1. It never edits the shipped unit file to fit your machine. Everything site
#      specific (workspace, data dir, port, binary path) goes into a drop-in at
#      /etc/systemd/system/karucode.service.d/10-install.conf, which is rewritten
#      on every run. Re-running the installer for an upgrade therefore cannot
#      lose the configuration that made the service start - which is the classic
#      way a systemd install "breaks on reboot after an update".
#      Put your own overrides in 20-local.conf next to it; install.sh never
#      touches that file.
#
#   2. Everything it changes is stated in the summary, and files it replaced are
#      kept as .bak / .old so a failed run can be undone. It creates the service
#      user, the workspace and the data directory; it does not re-own an existing
#      directory and never rewrites an existing environment file.
#
# Environment:
#   VERSION                   release to install, without the leading "v"
#   KARUCODE_REPO             owner/name to download from (default karutoil/karucode)
#   KARUCODE_RELEASE_BASE_URL override the download base (mirror, air-gapped host)
#   KARUCODE_RELEASE_API_URL  override the "latest release" endpoint
#   KARUCODE_UNIT_URL         override where the unit file is fetched from
#   KARUCODE_UNIT_DIR         override where the unit and its drop-in live
#                             (default /etc/systemd/system). The test suite uses
#                             this to point a dry run at a scratch directory
#                             instead of the host's real unit.
#
# Exit codes:
#   0  installed, and (when systemd was requested) the service is running
#   1  usage, download, verification or install failure - the unit, the drop-in
#      and the binary are rolled back to what was there before this run
#   2  the binary and unit are installed but the service did not stay up; see
#      the checklist printed at the end

set -eu

# restore_unit and finish only ever run from the EXIT trap; shellcheck cannot see
# that caller and calls their bodies unreachable.
# shellcheck disable=SC2317

# sudo and some container images hand an installer a PATH without sbin, where
# groupadd/useradd live. Look there regardless of how the script was started.
PATH="/usr/local/sbin:/usr/sbin:/sbin:${PATH:-/usr/bin:/bin}"
export PATH

REPO="${KARUCODE_REPO:-karutoil/karucode}"
VERSION="${VERSION:-latest}"
PREFIX="/usr/local"

# --- who the service runs as -----------------------------------------------
#
# The default is **the human who ran the installer**, not a separate system
# account and never root. That is the whole security model now that the unit does
# not sandbox the filesystem (see packaging/README.md): the agent gets your files,
# as you, and a dedicated account would only mean it can no longer reach your
# toolchains, `~/.cargo`, `~/.local/bin`, your projects, or the data directory it
# was told to use.
#
# Resolving "who ran this" is the part that is easy to get wrong, because `sudo`
# hands the script several different things and only one of them is the answer.
# The candidates are tried in this order:
#
#   $SUDO_USER    the account sudo switched *from* - the human. Set by sudo (and
#                 by `su -m`/`doas`-style wrappers); the primary signal.
#   $LOGNAME      the *login* name of the session. sudo resets $USER to `root` but
#                 leaves $LOGNAME alone, so `sudo sh install.sh` from a login shell
#                 still has LOGNAME=you even when $SUDO_USER is somehow absent.
#                 Touched only as root, so an unprivileged run can set it freely.
#   /proc/self/loginuid   the kernel's audit login uid of the session, resolved
#                 through passwd. Survives a `sudo -s` in a root shell and is set by
#                 sshd/login. Read only when it is a real uid and not the kernel's
#                 4294967295 "unset" sentinel.
#   $USER         the *effective* name. Under sudo this is `root`, which is exactly
#                 the trap: taking it first made `sudo sh install.sh` resolve to
#                 `root` and then fall back to the system account. It is trusted
#                 last, and only because a non-sudo run has no other evidence.
#
# The first name that resolves to a passwd entry that is not root/uid 0 wins. Every
# candidate goes through `is_human_account`, so a uid or a numeric gid can never be
# mistaken for an account name, and 0/root in any spelling is never the service
# account: the agent executes arbitrary commands, so root is refused outright
# rather than "used as a fallback".

# is_root_name NAME is true for every spelling of the superuser: the name `root`,
# the uid, a uid:gid pair, and the variants systemd accepts for it.
is_root_name() {
    case "$1" in
        ""|root|0|0:*|*:0|root:*) return 0 ;;
    esac
    # "0", "00", "0000": all digits, and none of them significant. Kept as a glob
    # rather than arithmetic so no shell's octal parsing can turn "010" into 8 (or
    # refuse "09") behind our back.
    case "$1" in
        *[!0-9]*) return 1 ;;
        *[1-9]*) return 1 ;;
        *) return 0 ;;
    esac
}

# is_human_account NAME is true when NAME is an existing account that is not root
# and has a real uid. A numeric name or a `name:group` pair is refused: systemd
# accepts `User=0` and `User=root:root` in a drop-in, so an unvalidated flag could
# smuggle root into the unit past the textual check below.
is_human_account() {
    case "$1" in
        ""|*:*) return 1 ;;
        *[!0-9]*) ;;
        *) return 1 ;;          # all digits -> a uid, not a name
    esac
    _uid="$(account_uid "$1")" || return 1
    [ -n "$_uid" ] || return 1
    [ "$_uid" -ne 0 ] || return 1
    return 0
}

# account_uid NAME prints the uid of an existing account, or fails. The single
# place that decides "does this account exist", so --service-user and the
# environment probes cannot disagree.
account_uid() {
    if command -v getent >/dev/null 2>&1; then
        _line="$(getent passwd "$1" 2>/dev/null)" || return 1
    else
        _line="$(awk -F: -v u="$1" '$1==u{print}' /etc/passwd 2>/dev/null)"
    fi
    [ -n "$_line" ] || return 1
    printf '%s\n' "$(printf '%s' "$_line" | cut -d: -f3)"
}

# login_uid prints the uid of the account that owns this session, or nothing.
# `4294967295` is the kernel's "unset" sentinel (no PAM login record).
login_uid() {
    _lu="$(cat /proc/self/loginuid 2>/dev/null || true)"
    case "$_lu" in
        ""|*[!0-9]*|4294967295) printf '%s\n' "" ;;
        *) printf '%s\n' "$_lu" ;;
    esac
}

# name_for_uid UID prints the account name owning UID, or nothing.
name_for_uid() {
    if command -v getent >/dev/null 2>&1; then
        getent passwd "$1" 2>/dev/null | cut -d: -f1
    else
        awk -F: -v u="$1" '$3==u{print $1; exit}' /etc/passwd 2>/dev/null
    fi
}

# resolve_default_user picks the account to run the service as. Empty means "no
# human could be identified", which the caller turns into the system-account
# fallback and a loud warning.
resolve_default_user() {
    for _candidate in "${SUDO_USER:-}" "${LOGNAME:-}" "$(name_for_uid "$(login_uid)")" "${USER:-}"; do
        if is_human_account "$_candidate"; then
            printf '%s\n' "$_candidate"
            return 0
        fi
    done
    printf '%s\n' ""
}

DEFAULT_USER="$(resolve_default_user)"

# SERVICE_USER is the account the unit names; SERVICE_GROUP follows later, once the
# flags are parsed, because it must come from the account's *real* gid rather than
# from an assumption that a group of the same name exists.
SYSTEM_USER="karucode"
SERVICE_USER="${DEFAULT_USER:-$SYSTEM_USER}"
SERVICE_GROUP=""
SERVICE_GROUP_EXPLICIT=0
# Set when the account was named explicitly (--service-user) rather than derived:
# the fallback warnings must not fire for a deliberate choice.
SERVICE_USER_EXPLICIT=0
SERVICE_NAME="karucode"
WORKSPACE=""
# DATA_DIR is resolved after the helper functions below: the account's own home for
# a system account, `<their home>/.local/share/karucode` for a person. An explicit
# --data-dir always wins.
DATA_DIR=""
DATA_DIR_EXPLICIT=0
HOST_ARG=""
PORT_ARG=""
ENV_FILE="/etc/karucode/karucode.env"

# UNIT_DIR is the real location on a host; KARUCODE_UNIT_DIR exists so the test
# suite can point a dry run at a scratch directory. It is not a flag: an operator
# who moved the unit wants --unit-file, and a flag here would be one more way to
# write a unit somewhere systemd never reads.
UNIT_DIR="${KARUCODE_UNIT_DIR:-/etc/systemd/system}"
UNIT_PATH="${UNIT_DIR}/${SERVICE_NAME}.service"
DROPIN_DIR="${UNIT_PATH}.d"
DROPIN_PATH="${DROPIN_DIR}/10-install.conf"

USE_SYSTEMD=1
ENABLE=1
START=1
RESTART=1
ALLOW_JIT=1
DRY_RUN=0
FORCE_SYSTEMD=0
ARTIFACT=""
UNIT_SRC=""

TMPDIR_=""
BIN_DIR=""
UNIT_BAK=""
DROPIN_BAK=""
BIN_BAK=""
BIN_INSTALLED=0
ROLLBACK=""

# --- output -----------------------------------------------------------------

die() {
    printf 'install.sh: error: %s\n' "$*" >&2
    exit 1
}

warn() {
    printf 'install.sh: warning: %s\n' "$*" >&2
}

info() {
    printf '==> %s\n' "$*"
}

# run CMD... executes, or prints the command when --dry-run is on. Every mutation
# goes through here, so a dry run is a faithful preview of the real one.
run() {
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '    (dry-run) %s\n' "$*"
    else
        "$@"
    fi
}

usage() {
    cat <<'EOF'
Install karucode from a GitHub release (or from a build you already have).

Usage: install.sh [OPTIONS]

Install source:
  --version <VER>        release to install, without the leading "v"
                         (default: "latest")
  --binary <PATH>        install this locally built binary instead of downloading
  --tarball <PATH>       install from a local release tarball instead of downloading

Layout:
  --prefix <DIR>         install the binary to <DIR>/bin (default: /usr/local)
  --workspace <DIR>      create this directory and start sessions in it.
                         Optional: without it the server starts with no project
                         and you add the first one in the web UI
  --data-dir <DIR>       SQLite database and plugin data. Default: the account's
                         own home for a system account (e.g. /var/lib/karucode),
                         else ~/.local/share/karucode
  --service-user <NAME>  run the service as this account. Default: the account
                         that ran this installer ($SUDO_USER, then $LOGNAME, then
                         the session login uid). Never created, never re-owned,
                         and never root. Pass a name only to override it.
  --service-group <GRP>  run it in this group (default: the account's real
                         primary group from /etc/passwd, not a guess)
  --host <ADDR>          bind address (default: whatever the unit sets, 127.0.0.1)
  --port <PORT>          listen port (default: 8787)
  --env-file <PATH>      environment file (default: /etc/karucode/karucode.env)

systemd:
  --no-systemd           install only the binary: no user, no unit, no systemctl
  --no-enable            install the unit but do not enable it at boot
  --no-start             do not start it now
  --no-restart           leave a running instance on the old binary
  --no-jit               keep MemoryDenyWriteExecute=yes (do not disable it).
                         On by default: without it node, Java and some Python
                         extensions crash, so the agent cannot build a frontend
  --unit-file <PATH>     install this unit file instead of the shipped/fetched one
  --force-systemd        proceed even if systemd does not look like PID 1
                         (a chroot or an image build)

Other:
  --dry-run              print every action and change nothing
  -h, --help             show this help

The systemd path needs root. --no-systemd works as any user whose --prefix/bin is
writable.

Environment: VERSION, KARUCODE_REPO, KARUCODE_RELEASE_BASE_URL,
KARUCODE_RELEASE_API_URL, KARUCODE_UNIT_URL
EOF
}

# --- small helpers ----------------------------------------------------------

need_value() {
    [ $# -ge 2 ] || die "$1 needs a value"
}

group_exists() {
    if command -v getent >/dev/null 2>&1; then
        getent group "$1" >/dev/null 2>&1
    else
        grep -q "^$1:" /etc/group 2>/dev/null
    fi
}

# user_exists NAME is true when NAME is an entry in passwd. It goes through
# account_uid so there is exactly one definition of "exists" in this script.
user_exists() {
    account_uid "$1" >/dev/null
}

# home_for USER prints that account's home directory, or nothing when it has none
# we can use. Only called for accounts that already exist, so a missing entry
# means "do not guess".
home_for() {
    if command -v getent >/dev/null 2>&1; then
        _home="$(getent passwd "$1" 2>/dev/null | cut -d: -f6)"
    else
        _home="$(awk -F: -v u="$1" '$1==u{print $6}' /etc/passwd 2>/dev/null)"
    fi
    case "$_home" in
        ""|/|/nonexistent) printf '%s\n' "" ;;
        *) printf '%s\n' "$_home" ;;
    esac
}

# account_group NAME prints the *name* of that account's primary group (its real
# gid, resolved through /etc/group), or nothing when the gid has no entry.
account_group() {
    _gid="$(account_gid "$1")"
    [ -n "$_gid" ] || return 1
    if command -v getent >/dev/null 2>&1; then
        getent group "$_gid" 2>/dev/null | cut -d: -f1
    else
        awk -F: -v g="$_gid" '$3==g{print $1; exit}' /etc/group 2>/dev/null
    fi
}

# account_gid NAME prints that account's numeric primary group id.
account_gid() {
    if command -v getent >/dev/null 2>&1; then
        _ent="$(getent passwd "$1" 2>/dev/null)" || return 1
    else
        _ent="$(awk -F: -v u="$1" '$1==u{print}' /etc/passwd 2>/dev/null)"
    fi
    [ -n "$_ent" ] || return 1
    printf '%s\n' "$(printf '%s' "$_ent" | cut -d: -f4)"
}

# is_system_account NAME is true for a service account rather than a person: no
# login shell, or a uid below the host's UID_MIN. It decides where the data
# directory goes, because the two kinds of account have different conventions - a
# person's app data belongs in ~/.local/share, a service account's belongs in the
# home useradd already gave it (that is what a system account's home *is*).
is_system_account() {
    account_uid "$1" >/dev/null || return 1
    _uid="$(account_uid "$1")"
    _shell="$(account_shell "$1")"
    case "$_shell" in
        */nologin|*/false|"") return 0 ;;
    esac
    _uid_min="$(awk '$1=="UID_MIN"{print $2; exit}' /etc/login.defs 2>/dev/null || true)"
    case "$_uid_min" in
        ''|*[!0-9]*) _uid_min=1000 ;;
    esac
    [ "$_uid" -lt "$_uid_min" ]
}

# account_shell NAME prints that account's login shell.
account_shell() {
    if command -v getent >/dev/null 2>&1; then
        _ent="$(getent passwd "$1" 2>/dev/null)" || return 1
    else
        _ent="$(awk -F: -v u="$1" '$1==u{print}' /etc/passwd 2>/dev/null)"
    fi
    [ -n "$_ent" ] || return 1
    printf '%s\n' "$(printf '%s' "$_ent" | cut -d: -f7)"
}

# --- read the configuration an earlier run wrote -----------------------------
#
# Re-running the installer is the upgrade path (packaging/README.md), and the
# drop-in is rewritten on every run. So a flag that is ABSENT on this run must not
# fall back to a built-in default: it has to fall back to the value the *previous*
# run recorded, or an upgrade silently repoints the data directory and the service
# comes up healthy on an empty database - sessions and the stored provider key
# simply gone. That was the bug this section exists to prevent.
#
# Precedence, and nothing else: an explicit flag > the line in the existing
# 10-install.conf > `systemctl show` on the loaded unit (which also sees
# 20-local.conf and the unit's own defaults) > the built-in default.
#
# Every read is defensive. The file is parsed by matching the exact keys this
# script writes, never sourced or eval'd, and a value that does not look like a
# path/port/account is discarded with a warning rather than trusted: a corrupted
# drop-in must degrade to today's behaviour, not to a directory the service cannot
# write.
#
# KNOWN LIMITATION, stated here because it decides how the values are resolved: the
# drop-in always writes a `WorkingDirectory=`. When no `--workspace` was given that
# line names the *data dir* (systemd needs some existing directory), so a data dir
# can be read back out of it but a workspace cannot be recovered from it - the two
# are indistinguishable in the file. A workspace is therefore carried over only
# when systemd can be asked directly (`systemctl show -p WorkingDirectory`).

# existing_dropin_value KEY prints the last `KEY=value` line in the drop-in an
# earlier run wrote, or nothing.
existing_dropin_value() {
    [ -f "$DROPIN_PATH" ] || return 0
    _edv="$(sed -n "s/^$1=//p" "$DROPIN_PATH" 2>/dev/null | tail -n 1)" || _edv=""
    printf '%s\n' "$_edv"
}

# existing_dropin_env NAME prints the value of `Environment=NAME=...` from the
# drop-in an earlier run wrote, or nothing. install.sh writes one Environment= line
# per variable, so this is a plain match on that exact shape.
existing_dropin_env() {
    [ -f "$DROPIN_PATH" ] || return 0
    _ede="$(sed -n "s/^Environment=$1=//p" "$DROPIN_PATH" 2>/dev/null | tail -n 1)" || _ede=""
    printf '%s\n' "$_ede"
}

# unit_prop NAME prints one property of the loaded unit, or nothing when systemd is
# absent, the unit is not installed, or the value is systemd's empty sentinel. Used
# only as the second-best source: the drop-in is what this script owns and what it
# is about to overwrite, so it is tried first.
unit_prop() {
    command -v systemctl >/dev/null 2>&1 || return 0
    [ -d /run/systemd/system ] || return 0
    _up="$(systemctl show "$SERVICE_NAME" -p "$1" 2>/dev/null | sed -n "s/^$1=//p" | head -n 1)" || _up=""
    case "$_up" in
        ""|[Nn]ot-found|"[not set]") _up="" ;;
    esac
    printf '%s\n' "$_up"
}

# unit_env NAME prints NAME=value out of the unit's merged Environment=, which
# systemd prints space-separated with values quoted when they contain spaces.
unit_env() {
    _ue_all="$(unit_prop Environment)"
    [ -n "$_ue_all" ] || return 0
    printf '%s\n' "$_ue_all" \
        | tr ' ' '\n' \
        | sed -n "s/^$1=//p" \
        | head -n 1 \
        | sed 's/^"//; s/"$//'
}

# is_abs_path PATH is true for an absolute path with no whitespace or newline in
# it: the only shape that can safely be written into a unit file unquoted.
is_abs_path() {
    case "$1" in
        /*) ;;
        *) return 1 ;;
    esac
    case "$1" in
        *[[:space:]]*) return 1 ;;
    esac
    return 0
}

# name_is_sane NAME rejects the values that would turn into a `User=`/`Group=`
# directive systemd cannot parse, or into something this script would then treat as
# an account it does not have.
name_is_sane() {
    case "$1" in
        ""|*[!A-Za-z0-9_.-]*) return 1 ;;
    esac
    return 0
}

# is_port PORT is true for a decimal port in range.
is_port() {
    case "$1" in
        ""|*[!0-9]*) return 1 ;;
    esac
    [ "$1" -ge 1 ] && [ "$1" -le 65535 ]
}
write_dropin() {
    printf '# Generated by packaging/install.sh - rewritten on every run.\n'
    printf '# Put your own overrides in 20-local.conf; install.sh never touches it.\n'
    printf '[Service]\n'
    # Always written, even when it matches the unit's own default: the resolved
    # account is the one fact an operator most needs to be able to check, and
    # `systemctl show -p User karucode` then cannot disagree with what this run
    # reported. Group= comes from the account's real primary group (see
    # --service-group), not from an assumption that name==group.
    printf 'User=%s\n' "$SERVICE_USER"
    printf 'Group=%s\n' "$SERVICE_GROUP"
    # systemd needs *some* existing WorkingDirectory; without a workspace the data
    # dir is the obvious neutral one. It is not a project: the operator creates the
    # first one in the web UI.
    if [ -n "$WORKSPACE" ]; then
        printf 'WorkingDirectory=%s\n' "$WORKSPACE"
    else
        printf 'WorkingDirectory=%s\n' "$DATA_DIR"
    fi
    # No ReadWritePaths= here: the shipped unit no longer sandboxes the filesystem,
    # so there is no allowlist to keep in sync. A leftover ReadWritePaths= from an
    # older install is cleared so it cannot re-introduce a restriction that no
    # longer has a UI.
    printf 'ReadWritePaths=\n'
    # The unit's StateDirectory=karucode creates and owns /var/lib/karucode as the
    # *service* user, which only works when that user is the dedicated system
    # account. Every other case - including the default of running as a real person
    # - points the server at a directory that user already owns, and clears
    # StateDirectory= so systemd does not try to chown a path it has no business
    # touching.
    #
    # The test is "is this the system account", not "is the path literally
    # /var/lib/karucode": the fallback account can be created with a different home,
    # and a person could pass --data-dir /var/lib/karucode. systemd resolves
    # StateDirectory= relative to /var/lib and would chown whatever it finds there
    # to the service user, so it may only stay when the service user is the account
    # the unit describes.
    if [ "$DATA_DIR" = "/var/lib/karucode" ] && [ "$SERVICE_USER" = "$SYSTEM_USER" ]; then
        : # the unit's own StateDirectory= already covers it
    else
        printf 'StateDirectory=\n'
        printf 'Environment=KARUCODE_DATA_DIR=%s\n' "$DATA_DIR"
    fi
    if [ -n "$HOST_ARG" ]; then
        printf 'Environment=KARUCODE_HOST=%s\n' "$HOST_ARG"
    fi
    if [ -n "$PORT_ARG" ]; then
        printf 'Environment=KARUCODE_PORT=%s\n' "$PORT_ARG"
    fi
    if [ "$ALLOW_JIT" -eq 1 ]; then
        # On by default: V8/JVM/Python C extensions need W^X memory, and without
        # this every `node`, `java` or `vite build` in a session dies with a V8
        # fatal error. `--no-jit` restores the knob.
        printf 'MemoryDenyWriteExecute=no\n'
    fi
    printf 'ExecStart=\n'
    printf 'ExecStart=%s/karucode\n' "$BIN_DIR"
}

# can_write PATH answers "could the service user write here?" from the mode bits
# alone. It never modifies the filesystem and never runs anything as that user.
can_write() {
    _path="$1"
    _meta="$(stat -c '%U %G %a' "$_path" 2>/dev/null)" || return 0
    _owner="${_meta%% *}"
    _rest="${_meta#* }"
    _group="${_rest%% *}"
    _mode="${_rest#* }"
    [ -n "$_mode" ] || return 0
    # 4-digit modes carry setuid/setgid/sticky; the permission triad is the tail.
    if [ "${#_mode}" -ge 4 ]; then
        _mode="${_mode#?}"
    fi
    _u=$(( (_mode / 100) % 10 ))
    _g=$(( (_mode / 10) % 10 ))
    _o=$(( _mode % 10 ))
    # write bit is 2: true for 2,3,6,7
    if [ "$_owner" = "$SERVICE_USER" ]; then
        [ $(( _u & 2 )) -eq 2 ]
        return $?
    fi
    if id -Gn "$SERVICE_USER" >/dev/null 2>&1; then
        if id -Gn "$SERVICE_USER" | tr ' ' '\n' | grep -qx "$_group"; then
            [ $(( _g & 2 )) -eq 2 ]
            return $?
        fi
    fi
    [ $(( _o & 2 )) -eq 2 ]
    return $?
}

# same_file A B is true when both paths name the same existing file. `install`
# refuses to copy a file onto itself, and "--binary the one that is already
# installed" is a reasonable thing for a caller to pass.
same_file() {
    [ -e "$1" ] && [ -e "$2" ] || return 1
    if command -v stat >/dev/null 2>&1; then
        [ "$(stat -c '%d:%i' "$1" 2>/dev/null)" = "$(stat -c '%d:%i' "$2" 2>/dev/null)" ]
        return $?
    fi
    [ "$1" = "$2" ]
}

# --- cleanup and rollback ---------------------------------------------------

# restore_unit puts back what was there before this run, so a broken install
# cannot leave a half-configured unit behind.
restore_unit() {
    [ -n "$ROLLBACK" ] || return 0
    warn "rolling back the systemd changes"
    if [ -n "$UNIT_BAK" ] && [ -f "$UNIT_BAK" ]; then
        cp -p "$UNIT_BAK" "$UNIT_PATH" && info "restored the previous ${UNIT_PATH}"
    else
        rm -f "$UNIT_PATH" && info "removed the unit this run installed"
    fi
    if [ -n "$DROPIN_BAK" ] && [ -f "$DROPIN_BAK" ]; then
        cp -p "$DROPIN_BAK" "$DROPIN_PATH" && info "restored the previous ${DROPIN_PATH}"
    else
        rm -f "$DROPIN_PATH"
    fi
    systemctl daemon-reload 2>/dev/null || true
    ROLLBACK=""
}

finish() {
    _rc=$?
    trap - EXIT INT TERM HUP
    if [ "$_rc" -ne 0 ]; then
        restore_unit
        if [ -n "$BIN_BAK" ] && [ -f "$BIN_BAK" ]; then
            warn "the previous binary is kept at ${BIN_BAK}"
            warn "roll back with: cp -p ${BIN_BAK} ${BIN_DIR}/karucode"
        fi
    fi
    if [ -n "$TMPDIR_" ] && [ -d "$TMPDIR_" ]; then
        rm -rf "$TMPDIR_"
    fi
    exit "$_rc"
}
trap finish EXIT INT TERM HUP

# --- argument parsing -------------------------------------------------------

while [ $# -gt 0 ]; do
    case "$1" in
        --prefix)          need_value "$@"; PREFIX="$2"; shift 2 ;;
        --prefix=*)        PREFIX="${1#*=}"; shift ;;
        --version)         need_value "$@"; VERSION="$2"; shift 2 ;;
        --version=*)       VERSION="${1#*=}"; shift ;;
        --workspace)       need_value "$@"; WORKSPACE="$2"; shift 2 ;;
        --workspace=*)     WORKSPACE="${1#*=}"; shift ;;
        --data-dir)        need_value "$@"; DATA_DIR="$2"; DATA_DIR_EXPLICIT=1; shift 2 ;;
        --data-dir=*)      DATA_DIR="${1#*=}"; DATA_DIR_EXPLICIT=1; shift ;;
        --service-user)    need_value "$@"; SERVICE_USER="$2"; SERVICE_USER_EXPLICIT=1; shift 2 ;;
        --service-user=*)  SERVICE_USER="${1#*=}"; SERVICE_USER_EXPLICIT=1; shift ;;
        --service-group)   need_value "$@"; SERVICE_GROUP="$2"; SERVICE_GROUP_EXPLICIT=1; shift 2 ;;
        --service-group=*) SERVICE_GROUP="${1#*=}"; SERVICE_GROUP_EXPLICIT=1; shift ;;
        --host)            need_value "$@"; HOST_ARG="$2"; shift 2 ;;
        --host=*)          HOST_ARG="${1#*=}"; shift ;;
        --port)            need_value "$@"; PORT_ARG="$2"; shift 2 ;;
        --port=*)          PORT_ARG="${1#*=}"; shift ;;
        --env-file)        need_value "$@"; ENV_FILE="$2"; shift 2 ;;
        --env-file=*)      ENV_FILE="${1#*=}"; shift ;;
        --unit-file)       need_value "$@"; UNIT_SRC="$2"; shift 2 ;;
        --unit-file=*)     UNIT_SRC="${1#*=}"; shift ;;
        --binary)          need_value "$@"; ARTIFACT="binary:$2"; shift 2 ;;
        --binary=*)        ARTIFACT="binary:${1#*=}"; shift ;;
        --tarball)         need_value "$@"; ARTIFACT="tarball:$2"; shift 2 ;;
        --tarball=*)       ARTIFACT="tarball:${1#*=}"; shift ;;
        --no-systemd)      USE_SYSTEMD=0; shift ;;
        --no-enable)       ENABLE=0; shift ;;
        --no-start)        START=0; shift ;;
        --no-restart)      RESTART=0; shift ;;
        --allow-jit)       ALLOW_JIT=1; shift ;;
        --no-jit)          ALLOW_JIT=0; shift ;;
        --force-systemd)   FORCE_SYSTEMD=1; shift ;;
        --dry-run)         DRY_RUN=1; shift ;;
        -h|--help)         usage; exit 0 ;;
        *)                 die "unknown argument: $1 (try --help)" ;;
    esac
done

[ -n "$PREFIX" ] || die "--prefix may not be empty"

# --- carry the previous run's configuration forward --------------------------
#
# Runs after the flags are parsed (an explicit flag always wins) and before
# anything is resolved from a default. --no-systemd is excluded: a binary-only
# install writes no drop-in and reads no unit, so there is nothing to carry.
#
# What is deliberately NOT carried: --prefix/--version/--binary (install-source and
# binary-location choices, not site configuration), and the booleans
# (--no-enable/--no-start/--no-restart/--no-jit). "Runs this time" is the only
# honest semantics for a flag, and a sticky --no-start would keep a machine that
# has been rebooted from ever coming back up.
#
# The workspace is the one value the drop-in cannot carry: its WorkingDirectory=
# line names the data dir whenever no workspace was given (systemd needs *some*
# existing directory), so a value read back could be the data dir. It is taken from
# the drop-in only when a data dir is also known and the two differ - the signature
# of a real workspace - and otherwise only from systemd.

PREV_USER=""
PREV_GROUP=""
PREV_DATA_DIR=""
PREV_HOST=""
PREV_PORT=""
# WorkingDirectory= has two possible sources and they are not interchangeable; both
# are read and the difference decides whether either is a workspace (below).
PREV_WD_FROM_DROPIN=""
PREV_WD_FROM_UNIT=""

if [ "$USE_SYSTEMD" -eq 1 ]; then
    PREV_USER="$(existing_dropin_value User)"
    PREV_GROUP="$(existing_dropin_value Group)"
    PREV_WD_FROM_DROPIN="$(existing_dropin_value WorkingDirectory)"
    PREV_DATA_DIR="$(existing_dropin_env KARUCODE_DATA_DIR)"
    PREV_HOST="$(existing_dropin_env KARUCODE_HOST)"
    PREV_PORT="$(existing_dropin_env KARUCODE_PORT)"
    PREV_WD_FROM_UNIT="$(unit_prop WorkingDirectory)"
    # NB: nothing else is read from the unit. The shipped unit is full of values
    # (User=, Environment=KARUCODE_DATA_DIR/HOST/PORT) and reading them back makes
    # "what the shipped unit says" indistinguishable from "what the previous install
    # chose": a dry run on a machine whose unit file has been hand-edited, or a
    # checkout whose unit changes, would then carry values nobody passed a flag for
    # and write them into the drop-in as if they were deliberate. The drop-in this
    # script writes is the only record of a previous *choice*; WorkingDirectory= is
    # the one exception, because the unit is where a hand-install keeps it.
fi

# A previous value is only usable if it has the shape this script would write.
# Anything else - a truncated file, a hand edit, a value with a `%` or a newline -
# is dropped with a warning, which lands the caller on today's default rather than
# on a path the service cannot write.
if [ -n "$PREV_DATA_DIR" ] && ! is_abs_path "$PREV_DATA_DIR"; then
    warn "the existing ${DROPIN_PATH} names a data dir that is not an absolute path: \"${PREV_DATA_DIR}\"; ignoring every recorded value in it"
    PREV_USER=""; PREV_GROUP=""; PREV_HOST=""; PREV_PORT=""
    PREV_WD_FROM_DROPIN=""; PREV_WD_FROM_UNIT=""
    PREV_DATA_DIR=""
fi
if [ -n "$PREV_WD_FROM_DROPIN" ] && ! is_abs_path "$PREV_WD_FROM_DROPIN"; then
    PREV_WD_FROM_DROPIN=""
fi
if [ -n "$PREV_WD_FROM_UNIT" ] && ! is_abs_path "$PREV_WD_FROM_UNIT"; then
    PREV_WD_FROM_UNIT=""
fi
if [ -n "$PREV_PORT" ] && ! is_port "$PREV_PORT"; then
    warn "ignoring the recorded port in the existing ${DROPIN_PATH} (\"${PREV_PORT}\" is not a port); using 8787"
    PREV_PORT=""
fi
if [ -n "$PREV_HOST" ]; then
    case "$PREV_HOST" in
        *[[:space:]]*|*%*) warn "ignoring the recorded bind address in the existing ${DROPIN_PATH} (\"${PREV_HOST}\")"; PREV_HOST="" ;;
    esac
fi
# The account is checked against passwd later (it is a *name*, and a name that no
# longer exists is worth a clear error rather than a unit that fails with 217/USER),
# so only its shape is checked here.
if [ -n "$PREV_USER" ] && ! name_is_sane "$PREV_USER"; then
    warn "ignoring the recorded service user in the existing ${DROPIN_PATH} (\"${PREV_USER}\"); using the default"
    PREV_USER=""
fi
if [ -n "$PREV_GROUP" ] && ! name_is_sane "$PREV_GROUP"; then
    warn "ignoring the recorded service group in the existing ${DROPIN_PATH} (\"${PREV_GROUP}\"); using ${PREV_USER:-the default}"
    PREV_GROUP=""
fi

# User and group are carried as a pair: a carried user with a *derived* group is not
# an upgrade of the previous configuration, it is a different one - and the account
# may not even exist any more.
if [ "$SERVICE_USER_EXPLICIT" -eq 0 ] && [ -n "$PREV_USER" ] && user_exists "$PREV_USER"; then
    SERVICE_USER="$PREV_USER"
    if [ "$SERVICE_GROUP_EXPLICIT" -eq 0 ] && [ -n "$PREV_GROUP" ] && group_exists "$PREV_GROUP"; then
        SERVICE_GROUP="$PREV_GROUP"
    fi
elif [ "$SERVICE_USER_EXPLICIT" -eq 0 ] && [ -n "$PREV_USER" ]; then
    warn "the service account recorded in ${DROPIN_PATH} (\"${PREV_USER}\") no longer exists; using the default instead"
fi

# The workspace, from systemd only (see the note above). An explicit --workspace
# always wins, and an explicit --data-dir suppresses the drop-in comparison entirely
# because the data dir in the file no longer describes this install.
if [ -z "$WORKSPACE" ]; then
    if [ -n "$PREV_WD_FROM_UNIT" ] \
        && [ "$PREV_WD_FROM_UNIT" != "$PREV_DATA_DIR" ] \
        && [ -d "$PREV_WD_FROM_UNIT" ]; then
        WORKSPACE="$PREV_WD_FROM_UNIT"
    elif [ -n "$PREV_WD_FROM_DROPIN" ] \
        && [ "$DATA_DIR_EXPLICIT" -eq 0 ] \
        && [ -n "$PREV_DATA_DIR" ] \
        && [ "$PREV_WD_FROM_DROPIN" != "$PREV_DATA_DIR" ] \
        && [ -d "$PREV_WD_FROM_DROPIN" ]; then
        WORKSPACE="$PREV_WD_FROM_DROPIN"
    fi
fi

if [ "$DATA_DIR_EXPLICIT" -eq 0 ] && [ -n "$PREV_DATA_DIR" ] \
    && [ -z "$PREV_USER" -o "$SERVICE_USER" = "$PREV_USER" ]; then
    DATA_DIR="$PREV_DATA_DIR"
    # Signals "carried over" to the data-dir resolution below, which must not
    # recompute it from the account's home.
    DATA_DIR_EXPLICIT=2
fi
if [ -z "$HOST_ARG" ] && [ -n "$PREV_HOST" ]; then
    HOST_ARG="$PREV_HOST"
fi
if [ -z "$PORT_ARG" ] && [ -n "$PREV_PORT" ]; then
    PORT_ARG="$PREV_PORT"
fi

# --- resolve the service account and its group ------------------------------
#
# Everything below reads SERVICE_USER as "a name that exists in passwd", so the
# checks are ordered: reject root in any spelling, then reject an account that does
# not exist (systemd would only fail later with status=217/USER), then derive the
# group from the account's real gid.
#
# Skipped for --no-systemd: a binary-only install writes no unit and runs nothing,
# so there is no service account to resolve and `sudo sh install.sh --no-systemd`
# must keep working on a host with no human accounts at all (an image build).

if [ "$USE_SYSTEMD" -eq 1 ]; then

    if is_root_name "$SERVICE_USER"; then
        die "the service must not run as root: the agent executes arbitrary commands.
Pick an unprivileged account that already exists, e.g. --service-user \$(id -un)."
    fi
    case "$SERVICE_USER" in
        *:*) die "--service-user takes an account name, not \"${SERVICE_USER}\" (use --service-group for the group)" ;;
    esac

    if [ "$SERVICE_USER_EXPLICIT" -eq 1 ]; then
        # An explicit name must be an account that exists. The installer does not
        # create accounts any more (the service runs as a person), and a typo would
        # otherwise reach systemd as status=217/USER long after the installer had
        # already reported success.
        if ! account_uid "$SERVICE_USER" >/dev/null; then
            die "no such account: ${SERVICE_USER}
The service runs as an account that already exists; this installer never creates one
for --service-user. Check the name with: id ${SERVICE_USER}"
        fi
    elif [ -z "$DEFAULT_USER" ]; then
        # No human could be identified: a bare `sudo -i` / container shell where
        # $SUDO_USER, $LOGNAME and loginuid all say root. Falling back is the only
        # way to install at all - but it must not be silent, because the agent will
        # not be able to reach the caller's home and the data dir moves to
        # /var/lib/karucode.
        warn "could not identify the human installing this: no usable \$SUDO_USER, no"
        warn "usable \$LOGNAME, and the session login uid is root. This looks like a bare"
        warn "root shell or an image build. Falling back to the dedicated system account"
        warn "\"${SERVICE_USER}\" (created if missing); pass --service-user <name> to"
        warn "choose the account yourself, or run the installer as the human who should"
        warn "own the service:  sudo -u <you> env SUDO_USER=<you> sh ${0##*/}"
    fi

    # Derive the group from the account's real gid unless --service-group was given.
    # Assuming a group named after the account is wrong on any host with user-private
    # groups disabled, or for an account whose primary group is shared (`users`,
    # `nogroup`, a project group): systemd then refuses to start with "Failed to
    # determine group credentials", or the unit runs in a group nobody intended.
    if [ "$SERVICE_GROUP_EXPLICIT" -eq 1 ]; then
        group_exists "$SERVICE_GROUP" \
            || die "no such group: ${SERVICE_GROUP}. Create it first, or drop --service-group to use ${SERVICE_USER}'s primary group."
    elif account_uid "$SERVICE_USER" >/dev/null; then
        SERVICE_GROUP="$(account_group "$SERVICE_USER")"
        [ -n "$SERVICE_GROUP" ] || SERVICE_GROUP="$SERVICE_USER"
    else
        # The account does not exist yet (the system-account fallback creates it), so
        # there is no gid to read; its same-name group is created alongside it.
        SERVICE_GROUP="$SERVICE_USER"
    fi
fi

# Resolve the data directory now that the flags are parsed and the helpers exist.
# A real user's own home is the default because that is the one place the service
# is guaranteed to be able to write without a separate account, a StateDirectory=
# or an ACL.
#
# Only an account with a *usable* home gets the ~/.local/share default. The
# fallback system account has a home of /var/lib/karucode already (useradd
# --home-dir), and appending .local/share to that produces the nonsensical
# /var/lib/karucode/.local/share/karucode - the historical default of /var/lib/
# karucode is the data dir for exactly that account, and it is also the path the
# unit's StateDirectory=karucode creates.
if [ "$DATA_DIR_EXPLICIT" -eq 0 ]; then
    _home="$(home_for "$SERVICE_USER")"
    if [ -z "$_home" ]; then
        DATA_DIR="/var/lib/karucode"
    elif is_system_account "$SERVICE_USER"; then
        # A service account's home is already its data directory (that is the
        # convention useradd --system sets up, and what the shipped unit's
        # StateDirectory=karucode describes). Appending .local/share would both
        # invent a path nobody expects and reproduce the bug this replaced:
        # /var/lib/karucode/.local/share/karucode.
        DATA_DIR="$_home"
    else
        DATA_DIR="${_home}/.local/share/karucode"
    fi
fi
[ -n "$DATA_DIR" ] || die "--data-dir may not be empty"
if [ -n "$PORT_ARG" ]; then
    case "$PORT_ARG" in
        *[!0-9]*) die "--port must be a number, got: $PORT_ARG" ;;
    esac
    if [ "$PORT_ARG" -lt 1 ] || [ "$PORT_ARG" -gt 65535 ]; then
        die "--port must be between 1 and 65535, got: $PORT_ARG"
    fi
fi
if [ -n "$UNIT_SRC" ] && [ ! -f "$UNIT_SRC" ]; then
    die "--unit-file does not exist: ${UNIT_SRC}"
fi

# --- platform ---------------------------------------------------------------

[ "$(uname -s)" = "Linux" ] || die "karucode releases are Linux binaries; this is $(uname -s). Build from source: cargo build --release -p kc-server"

case "$(uname -m)" in
    x86_64|amd64)
        TARGET="x86_64-unknown-linux-gnu"
        ;;
    aarch64|arm64)
        # The release workflow publishes x86_64 only today. The name is still
        # correct, so this works as soon as an aarch64 artifact exists.
        TARGET="aarch64-unknown-linux-gnu"
        warn "no aarch64 release artifact is published yet; the download will probably 404."
        warn "Build from source and pass the result instead:"
        warn "    cargo build --release -p kc-server && sudo sh install.sh --binary target/release/karucode"
        ;;
    *)
        die "unsupported architecture: $(uname -m). Build from source: cargo build --release -p kc-server"
        ;;
esac

# --- privileges and systemd availability ------------------------------------

if [ "$USE_SYSTEMD" -eq 1 ]; then
    if [ "$DRY_RUN" -eq 0 ] && [ "$(id -u)" -ne 0 ]; then
        die "installing the systemd unit needs root: re-run with sudo, or pass --no-systemd to install only the binary"
    fi
    if [ ! -d /run/systemd/system ] && [ "$FORCE_SYSTEMD" -eq 0 ]; then
        warn "systemd does not look like the init system here (no /run/systemd/system)."
        warn "That is normal inside a container or an image build. Falling back to --no-systemd."
        warn "Pass --force-systemd to write the unit files anyway."
        USE_SYSTEMD=0
    fi
    if [ "$USE_SYSTEMD" -eq 1 ] && ! command -v systemctl >/dev/null 2>&1; then
        warn "systemctl not found; skipping the unit. Only the binary will be installed."
        USE_SYSTEMD=0
    fi
fi
if [ "$USE_SYSTEMD" -eq 0 ]; then
    ENABLE=0
    START=0
    RESTART=0
fi

BIN_DIR="${PREFIX}/bin"
if [ "$DRY_RUN" -eq 0 ]; then
    if [ ! -d "$BIN_DIR" ]; then
        mkdir -p "$BIN_DIR" 2>/dev/null || die "cannot create ${BIN_DIR} (permission denied? try sudo)"
    fi
    if [ ! -w "$BIN_DIR" ]; then
        die "${BIN_DIR} is not writable (try sudo, or --prefix)"
    fi
fi

# --- downloader / verifier --------------------------------------------------

if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL --retry 3 --connect-timeout 15 -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -q -O "$2" "$1"; }
else
    die "neither curl nor wget is available"
fi

if command -v sha256sum >/dev/null 2>&1; then
    sha256_of() { sha256sum "$1" | awk '{print $1}'; }
elif command -v shasum >/dev/null 2>&1; then
    sha256_of() { shasum -a 256 "$1" | awk '{print $1}'; }
else
    die "neither sha256sum nor shasum is available; cannot verify the download"
fi

# --- resolve the version ----------------------------------------------------

TMPDIR_="$(mktemp -d)" || die "cannot create a temporary directory"

if [ -n "$ARTIFACT" ]; then
    if [ "$VERSION" = "latest" ]; then
        VERSION=""
        case "$ARTIFACT" in
            binary:*)
                _candidate="${ARTIFACT#binary:}"
                if [ -f "$_candidate" ]; then
                    VERSION="$("$_candidate" --version 2>/dev/null | awk '{print $NF}')" || VERSION=""
                fi
                ;;
        esac
        if [ -z "$VERSION" ]; then
            VERSION="local"
        fi
    fi
else
    if [ "$VERSION" = "latest" ]; then
        info "resolving the latest release of ${REPO}"
        api="${KARUCODE_RELEASE_API_URL:-https://api.github.com/repos/${REPO}/releases/latest}"
        api_file="${TMPDIR_}/release.json"
        tag=""
        if fetch "$api" "$api_file" 2>/dev/null; then
            tag="$(sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$api_file" | head -n 1)"
        fi
        rm -f "$api_file"
        if [ -z "$tag" ]; then
            die "could not resolve the latest release of ${REPO} (rate limited? no network? pass --version <VER>)"
        fi
        VERSION="${tag#v}"
    fi
fi
VERSION="${VERSION#v}"
[ -n "$VERSION" ] || die "empty version"

NAME="karucode-${VERSION}-${TARGET}"
TARBALL="${NAME}.tar.gz"
BASE_URL="${KARUCODE_RELEASE_BASE_URL:-https://github.com/${REPO}/releases/download/v${VERSION}}"
UNIT_URL="${KARUCODE_UNIT_URL:-https://raw.githubusercontent.com/${REPO}/v${VERSION}/packaging/karucode.service}"

# --- obtain the artifact ----------------------------------------------------

case "$ARTIFACT" in
    "")
        info "downloading ${TARBALL}"
        fetch "${BASE_URL}/${TARBALL}" "${TMPDIR_}/${TARBALL}" \
            || die "download failed: ${BASE_URL}/${TARBALL}
No artifact for ${TARGET} in release v${VERSION}? See https://github.com/${REPO}/releases"

        info "downloading SHA256SUMS"
        fetch "${BASE_URL}/SHA256SUMS" "${TMPDIR_}/SHA256SUMS" \
            || die "could not download ${BASE_URL}/SHA256SUMS; refusing to install an unverified binary"

        expected="$(awk -v f="$TARBALL" '$2 == f || $2 == "*" f {print $1; exit}' "${TMPDIR_}/SHA256SUMS")"
        [ -n "$expected" ] || die "${TARBALL} is not listed in SHA256SUMS; refusing to install an unverified binary"
        actual="$(sha256_of "${TMPDIR_}/${TARBALL}")"
        if [ "$expected" != "$actual" ]; then
            die "checksum mismatch for ${TARBALL}
  expected: ${expected}
  actual:   ${actual}
The download is corrupt or has been tampered with. Nothing was installed."
        fi
        info "checksum verified (${actual})"

        command -v tar >/dev/null 2>&1 || die "tar is required to unpack ${TARBALL}"
        tar -xzf "${TMPDIR_}/${TARBALL}" -C "$TMPDIR_" || die "could not unpack ${TARBALL}"
        NEW_BINARY="${TMPDIR_}/${NAME}/karucode"
        if [ ! -f "$NEW_BINARY" ]; then
            # Tolerate a tarball with a different top-level directory name.
            NEW_BINARY="$(find "$TMPDIR_" -type f -name karucode | head -n 1)"
        fi
        ;;
    binary:*)
        src="${ARTIFACT#binary:}"
        [ -f "$src" ] || die "--binary path does not exist: ${src}"
        [ -x "$src" ] || die "--binary is not executable: ${src}"
        NEW_BINARY="$src"
        info "using the local build ${src}"
        ;;
    tarball:*)
        src="${ARTIFACT#tarball:}"
        [ -f "$src" ] || die "--tarball path does not exist: ${src}"
        command -v tar >/dev/null 2>&1 || die "tar is required to unpack ${src}"
        info "using the local tarball ${src}"
        # Verify a local tarball against a sibling SHA256SUMS when one is there,
        # so the offline path is as honest as the downloaded one.
        sums="$(dirname "$src")/SHA256SUMS"
        if [ -f "$sums" ]; then
            base="$(basename "$src")"
            expected="$(awk -v f="$base" '$2 == f || $2 == "*" f {print $1; exit}' "$sums")"
            if [ -n "$expected" ]; then
                actual="$(sha256_of "$src")"
                if [ "$expected" != "$actual" ]; then
                    die "checksum mismatch for ${base}
  expected: ${expected}
  actual:   ${actual}
Nothing was installed."
                fi
                info "checksum verified (${actual})"
            fi
        fi
        tar -xzf "$src" -C "$TMPDIR_" || die "could not unpack ${src}"
        NEW_BINARY="$(find "$TMPDIR_" -type f -name karucode | head -n 1)"
        ;;
    *)
        die "internal error: unknown artifact ${ARTIFACT}"
        ;;
esac

if [ -z "${NEW_BINARY:-}" ] || [ ! -f "$NEW_BINARY" ]; then
    die "no karucode binary found in the artifact"
fi
chmod 0755 "$NEW_BINARY" 2>/dev/null || true

# Smoke-test the artifact before it can touch /usr/local: a binary built for the
# wrong libc should fail here, not on the next boot.
if [ "$DRY_RUN" -eq 0 ]; then
    if ! "$NEW_BINARY" --version >/dev/null 2>&1; then
        die "${NEW_BINARY} does not run on this machine (wrong architecture or missing libc). Nothing was installed."
    fi
fi

# --- preflight --------------------------------------------------------------
# The most common reason a fresh install "just doesn't work" is a hand-started
# instance already holding the port: the unit starts, the bind fails, and the only
# evidence is one line in the journal.

if [ "$USE_SYSTEMD" -eq 1 ] && [ "$DRY_RUN" -eq 0 ]; then
    if [ -n "$PORT_ARG" ] && command -v ss >/dev/null 2>&1; then
        if ! systemctl is-active --quiet "$SERVICE_NAME" 2>/dev/null; then
            if ss -Hltn "sport = :${PORT_ARG}" 2>/dev/null | grep -q .; then
                warn "port ${PORT_ARG} is already in use."
                warn "If that is a hand-started karucode, stop it before starting the service:"
                warn "    ss -ltnp | grep :${PORT_ARG}   # find the pid, stop that process"
                warn "or install with a different --port."
            fi
        fi
    fi
    if [ -n "$WORKSPACE" ] && [ -d "$WORKSPACE" ] && ! can_write "$WORKSPACE"; then
        warn "${WORKSPACE} exists but ${SERVICE_USER} cannot write to it."
        warn "The agent will fail on the first edit. Fix it with:"
        warn "    chown -R ${SERVICE_USER}:${SERVICE_GROUP} ${WORKSPACE}"
    fi
fi

# --- install the binary -----------------------------------------------------

if [ -f "${BIN_DIR}/karucode" ] && [ "$DRY_RUN" -eq 0 ]; then
    BIN_BAK="${BIN_DIR}/karucode.old"
    cp -p "${BIN_DIR}/karucode" "$BIN_BAK" || warn "could not keep a copy of the previous binary"
fi
if same_file "$NEW_BINARY" "${BIN_DIR}/karucode"; then
    info "${BIN_DIR}/karucode is already this file; leaving it in place"
else
    if [ "$DRY_RUN" -eq 1 ]; then
        info "would install ${BIN_DIR}/karucode"
    else
        info "installing ${BIN_DIR}/karucode"
    fi
    run install -m 0755 "$NEW_BINARY" "${BIN_DIR}/karucode" \
        || die "could not install the binary to ${BIN_DIR}"
fi

# --- systemd: user, directories, env file, unit, drop-in --------------------

was_active="inactive"
if [ "$USE_SYSTEMD" -eq 1 ] && [ "$DRY_RUN" -eq 0 ]; then
    was_active="$(systemctl is-active "$SERVICE_NAME" 2>/dev/null || true)"
    if [ -z "$was_active" ]; then
        was_active="inactive"
    fi
fi

if [ "$USE_SYSTEMD" -eq 1 ]; then
    if user_exists "$SERVICE_USER"; then
        # The normal path: the service runs as a real account that already exists
        # (the person who ran the installer). Never create, never re-own.
        if [ "$SERVICE_USER_EXPLICIT" -eq 0 ] && [ -n "$DEFAULT_USER" ]; then
            info "running as ${SERVICE_USER}:${SERVICE_GROUP} (the account that ran this installer)"
        elif [ "$SERVICE_USER_EXPLICIT" -eq 0 ]; then
            info "running as ${SERVICE_USER}:${SERVICE_GROUP} (system-account fallback)"
        else
            info "running as the existing account ${SERVICE_USER}:${SERVICE_GROUP}"
        fi
        # An account with no real home would put the data dir somewhere unwritable;
        # say so rather than fail later.
        if [ -z "$(home_for "$SERVICE_USER")" ] && [ "$DATA_DIR_EXPLICIT" -eq 0 ]; then
            warn "${SERVICE_USER} has no usable home directory; install with --data-dir <path>."
        fi
    else
        # Only reachable for the built-in fallback: --service-user is validated to
        # name an existing account, so nothing here can create an arbitrary account.
        info "creating the ${SERVICE_USER} system group and user (fallback: no human identified)"
        # The unit names Group= explicitly, so create the group explicitly too.
        # Relying on useradd's implicit same-name group (USERGROUPS_ENAB) is how
        # this breaks on hosts that have it disabled: systemd then refuses to
        # start with "Failed to determine group credentials".
        if ! group_exists "$SERVICE_GROUP"; then
            if command -v groupadd >/dev/null 2>&1; then
                run groupadd --system "$SERVICE_GROUP" || die "could not create the ${SERVICE_GROUP} group"
            elif command -v addgroup >/dev/null 2>&1; then
                run addgroup --system "$SERVICE_GROUP" || die "could not create the ${SERVICE_GROUP} group"
            else
                die "no groupadd/addgroup available to create the ${SERVICE_GROUP} group"
            fi
        fi
        nologin="/usr/sbin/nologin"
        if [ ! -x "$nologin" ]; then
            nologin="/sbin/nologin"
        fi
        if [ ! -x "$nologin" ]; then
            nologin="/bin/false"
        fi
        if command -v useradd >/dev/null 2>&1; then
            run useradd --system --gid "$SERVICE_GROUP" --home-dir "$DATA_DIR" \
                --no-create-home --shell "$nologin" "$SERVICE_USER" \
                || die "could not create the ${SERVICE_USER} user"
        elif command -v adduser >/dev/null 2>&1; then
            run adduser --system --ingroup "$SERVICE_GROUP" --home "$DATA_DIR" \
                --shell "$nologin" --disabled-login "$SERVICE_USER" \
                || die "could not create the ${SERVICE_USER} user"
        else
            die "no useradd/adduser available to create the ${SERVICE_USER} user"
        fi
    fi

    # Workspace: create it when missing (that is what makes the unit startable),
    # never re-own one that already exists.
    if [ -n "$WORKSPACE" ] && [ ! -d "$WORKSPACE" ]; then
        info "creating the workspace ${WORKSPACE}"
        run install -d -m 0750 -o "$SERVICE_USER" -g "$SERVICE_GROUP" "$WORKSPACE" \
            || die "could not create the workspace ${WORKSPACE}"
    fi

    # Data dir: StateDirectory= does this for the default path; a custom
    # --data-dir has to be created and owned here.
    if [ ! -d "$DATA_DIR" ]; then
        info "creating the data directory ${DATA_DIR}"
        run install -d -m 0700 -o "$SERVICE_USER" -g "$SERVICE_GROUP" "$DATA_DIR" \
            || die "could not create the data directory ${DATA_DIR}"
    elif [ "$DRY_RUN" -eq 0 ] && ! can_write "$DATA_DIR"; then
        warn "${DATA_DIR} exists but ${SERVICE_USER} cannot write to it; the server will fail at startup."
        warn "    chown -R ${SERVICE_USER}:${SERVICE_GROUP} ${DATA_DIR}"
    fi

    # Environment file: create a commented template when absent, so
    # EnvironmentFile= has something to read and there is one obvious place for
    # the model credentials. An existing file is never touched.
    if [ ! -f "$ENV_FILE" ]; then
        info "writing a template environment file at ${ENV_FILE}"
        if [ "$DRY_RUN" -eq 0 ]; then
            install -d -m 0755 "$(dirname "$ENV_FILE")"
            ( umask 077; : > "$ENV_FILE" ) || die "could not create ${ENV_FILE}"
            chmod 0600 "$ENV_FILE"
            cat > "$ENV_FILE" <<'ENVEOF'
# karucode environment - read by systemd for the karucode.service unit.
# Mode 0600, owned by root. Restart the service after editing:
#     sudo systemctl restart karucode
#
# Model connection (any OpenAI-compatible gateway). Without these the server
# starts and serves the UI, but every turn fails.
#KARUCODE_BASE_URL=https://api.example.com/v1
#KARUCODE_API_KEY=sk-...
#KARUCODE_MODEL=gpt-4o-mini
#
# Required once anything other than this host can reach the port, and required
# when KARUCODE_HOST is not a loopback address (>= 16 chars):
#KARUCODE_AUTH_TOKEN=
#
# Optional: pin the settings the UI is not allowed to change.
#KARUCODE_LOCK_SETTINGS=1
ENVEOF
        else
            printf '    (dry-run) write a template environment file to %s (mode 0600)\n' "$ENV_FILE"
        fi
    fi

    # Unit file. Keep whatever was there so a failure can be undone.
    if [ -f "$UNIT_PATH" ]; then
        UNIT_BAK="${UNIT_PATH}.bak"
        if [ "$DRY_RUN" -eq 0 ]; then
            cp -p "$UNIT_PATH" "$UNIT_BAK" || warn "could not back up the existing unit"
        fi
    fi
    if [ -f "$DROPIN_PATH" ]; then
        DROPIN_BAK="${DROPIN_PATH}.bak"
        if [ "$DRY_RUN" -eq 0 ]; then
            cp -p "$DROPIN_PATH" "$DROPIN_BAK" || warn "could not back up the existing drop-in"
        fi
    fi

    ROLLBACK=1

    info "installing ${UNIT_PATH}"
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '    (dry-run) install the unit to %s\n' "$UNIT_PATH"
    elif [ -n "$UNIT_SRC" ]; then
        if same_file "$UNIT_SRC" "$UNIT_PATH"; then
            info "${UNIT_PATH} is already this file; leaving it in place"
        else
            install -m 0644 "$UNIT_SRC" "$UNIT_PATH" || die "could not install ${UNIT_SRC} to ${UNIT_PATH}"
        fi
    else
        # Prefer the unit shipped next to this script (a source checkout); fall
        # back to the copy in the tagged release, so the file installed as root
        # is pinned to the version being installed rather than to "main".
        script_dir="$(
            unset CDPATH
            cd -- "$(dirname -- "$0")" 2>/dev/null && pwd
        )" || script_dir=""
        if [ -n "$script_dir" ] && [ -f "${script_dir}/${SERVICE_NAME}.service" ]; then
            install -m 0644 "${script_dir}/${SERVICE_NAME}.service" "$UNIT_PATH" \
                || die "could not install the unit to ${UNIT_PATH}"
        else
            fetch "$UNIT_URL" "${TMPDIR_}/unit" \
                || die "could not download the unit from ${UNIT_URL} (or pass --unit-file)"
            grep -q '^\[Unit\]' "${TMPDIR_}/unit" || die "${UNIT_URL} did not return a systemd unit"
            install -m 0644 "${TMPDIR_}/unit" "$UNIT_PATH" \
                || die "could not install the unit to ${UNIT_PATH}"
        fi
    fi

    # Site configuration lives in a drop-in, so re-running install.sh for an
    # upgrade cannot lose the paths the service needs in order to start.
    info "writing ${DROPIN_PATH}"
    dropin_tmp="${TMPDIR_}/10-install.conf"
    write_dropin > "$dropin_tmp"
    if [ "$DRY_RUN" -eq 1 ]; then
        # Print the real file, not a description of it: the resolved User=/Group=
        # and data dir are the two facts this script can get wrong silently, and a
        # dry run is the only chance to check them before a unit is installed.
        printf '    (dry-run) would write %s:\n' "$DROPIN_PATH"
        sed 's/^/    | /' "$dropin_tmp"
    else
        install -d -m 0755 "$DROPIN_DIR"
        install -m 0644 "$dropin_tmp" "$DROPIN_PATH" \
            || die "could not write ${DROPIN_PATH}"
    fi

    info "systemctl daemon-reload"
    run systemctl daemon-reload || die "systemctl daemon-reload failed"

    if [ "$ENABLE" -eq 1 ]; then
        info "enabling ${SERVICE_NAME} at boot"
        run systemctl enable "$SERVICE_NAME" \
            || die "systemctl enable ${SERVICE_NAME} failed"
    else
        warn "not enabling ${SERVICE_NAME}: it will not start after a reboot"
    fi
fi

# --- start / restart --------------------------------------------------------

DID_START=0
STARTED=0
ENABLED_STATE=""
if [ "$USE_SYSTEMD" -eq 1 ] && [ "$DRY_RUN" -eq 0 ]; then
    # A unit that failed earlier (bad env, busy port) is held back by
    # StartLimitBurst. Without this, re-running the installer to fix the problem
    # is refused with "start of the service was attempted too often" until
    # someone resets the failure counter by hand.
    systemctl reset-failed "$SERVICE_NAME" 2>/dev/null || true

    if [ "$was_active" = "active" ] && [ "$RESTART" -eq 1 ]; then
        info "restarting ${SERVICE_NAME} on the new binary"
        systemctl restart "$SERVICE_NAME" || true
        DID_START=1
    elif [ "$START" -eq 1 ]; then
        info "starting ${SERVICE_NAME}"
        systemctl start "$SERVICE_NAME" || true
        DID_START=1
    fi

    if [ "$DID_START" -eq 1 ]; then
        # Type=simple means systemd reports "active" the moment it forks, so a
        # single is-active probe happily reports success for a process that is
        # about to exit (a busy port, a bad env file). Require the state to hold
        # for a few consecutive seconds before believing it.
        _i=0
        _streak=0
        while [ "$_i" -lt 25 ]; do
            _state="$(systemctl is-active "$SERVICE_NAME" 2>/dev/null || true)"
            if [ "$_state" = "active" ]; then
                _streak=$((_streak + 1))
                if [ "$_streak" -ge 3 ]; then
                    STARTED=1
                    break
                fi
            else
                _streak=0
                if [ "$_state" = "failed" ]; then
                    break
                fi
            fi
            _i=$((_i + 1))
            sleep 1
        done
    fi

    ENABLED_STATE="$(systemctl is-enabled "$SERVICE_NAME" 2>/dev/null || true)"
fi

# --- health probe -----------------------------------------------------------

health_line=""
if [ "$USE_SYSTEMD" -eq 1 ] && [ "$DRY_RUN" -eq 0 ] && [ "$STARTED" -eq 1 ]; then
    probe_host="${HOST_ARG:-127.0.0.1}"
    case "$probe_host" in
        0.0.0.0|'*'|'::'|'[::]') probe_host="127.0.0.1" ;;
    esac
    probe_port="${PORT_ARG:-8787}"
    if command -v curl >/dev/null 2>&1; then
        # `systemctl start` returns as soon as the process is spawned
        # (Type=simple) and binding the port happens a moment later, so poll
        # instead of probing once and reporting a race as a failure.
        code=""
        _tries=0
        while [ "$_tries" -lt 15 ]; do
            code="$(curl -s -o /dev/null -m 3 -w '%{http_code}' "http://${probe_host}:${probe_port}/api/health" 2>/dev/null)" || code=""
            case "$code" in
                200|401|403) break ;;
            esac
            _tries=$((_tries + 1))
            sleep 1
        done
        case "$code" in
            200) health_line="ok (HTTP 200 on http://${probe_host}:${probe_port}/api/health)" ;;
            401) health_line="ok (HTTP 401: up, and /api/health wants a token)" ;;
            403) health_line="up, but the Host header was refused (HTTP 403); see KARUCODE_ALLOWED_HOSTS" ;;
            '')  health_line="no answer on http://${probe_host}:${probe_port}/api/health" ;;
            *)   health_line="unexpected HTTP ${code} on http://${probe_host}:${probe_port}/api/health" ;;
        esac
    fi
fi

ROLLBACK=""

# --- summary ----------------------------------------------------------------

INSTALLED_VERSION="karucode ${VERSION}"
if [ "$DRY_RUN" -eq 0 ] && [ -x "${BIN_DIR}/karucode" ]; then
    _v="$("${BIN_DIR}/karucode" --version 2>/dev/null || true)"
    if [ -n "$_v" ]; then
        INSTALLED_VERSION="$_v"
    fi
fi

printf '\n%s\n' "$([ "$DRY_RUN" -eq 1 ] && printf 'would install' || printf 'installed') ${INSTALLED_VERSION} to ${BIN_DIR}/karucode"

# The resolved account is the single most checkable fact about an install: it is
# what the agent will be able to read and write, and it is the thing a wrong
# SUDO_USER gets wrong silently. Printed before the file list so it cannot be
# missed, and worded so it matches `systemctl show -p User,Group`.
if [ "$USE_SYSTEMD" -eq 1 ]; then
    printf 'Service account: %s:%s' "$SERVICE_USER" "$SERVICE_GROUP"
    if [ "$SERVICE_USER_EXPLICIT" -eq 0 ] && [ -n "$DEFAULT_USER" ]; then
        printf ' (who ran this installer)'
    elif [ "$SERVICE_USER_EXPLICIT" -eq 0 ]; then
        printf ' (fallback: no human could be identified)'
    fi
    printf '\nData directory:  %s\n' "$DATA_DIR"
fi

if [ "$DRY_RUN" -eq 1 ]; then
    printf '\nDry run: nothing was downloaded, installed or written.\n\n'
    printf 'A real run would write %s/karucode and:\n' "$BIN_DIR"
    if [ "$USE_SYSTEMD" -eq 1 ]; then
        printf '  %s\n' "$UNIT_PATH"
        printf '  %s\n' "$DROPIN_PATH"
        printf '  %s (only if it does not exist yet)\n' "$ENV_FILE"
        if [ -n "$WORKSPACE" ]; then
            printf '  %s and %s (only if they do not exist yet)\n' "$WORKSPACE" "$DATA_DIR"
        else
            printf '  %s (only if it does not exist yet)\n' "$DATA_DIR"
        fi
        if user_exists "$SERVICE_USER"; then
            printf '  the %s account is used as-is (never created, never re-owned)\n' "$SERVICE_USER"
        else
            printf '  the %s system user and group (only if missing)\n' "$SERVICE_USER"
        fi
        printf '  and then: systemctl daemon-reload, enable, and start/restart %s\n' "$SERVICE_NAME"
    else
        printf '  (no systemd unit: --no-systemd or systemd was not detected)\n'
    fi
    exit 0
fi

if [ "$USE_SYSTEMD" -ne 1 ]; then
    cat <<EOF

Next steps (no systemd, nothing was enabled):

  ${BIN_DIR}/karucode        # then add a project at http://127.0.0.1:8787

Set KARUCODE_DATA_DIR, KARUCODE_AUTH_TOKEN and the model variables first; see
packaging/README.md.
EOF
    exit 0
fi

if [ "$STARTED" -eq 1 ]; then
    printf 'Service: active'
    if [ "$ENABLED_STATE" = "enabled" ]; then
        printf ', enabled at boot'
    else
        printf ', NOT enabled at boot (enabled state: %s)' "${ENABLED_STATE:-unknown}"
    fi
    if [ -n "$health_line" ]; then
        printf ' - %s' "$health_line"
    fi
    printf '\n'
    cat <<EOF

Logs:      journalctl -u ${SERVICE_NAME} -f
Status:    systemctl status ${SERVICE_NAME}
Restart:   sudo systemctl restart ${SERVICE_NAME}
Env file:  ${ENV_FILE}  (mode 0600; never overwritten by this script)

Configure the model before the first real turn:
  sudoedit ${ENV_FILE}     # KARUCODE_BASE_URL / KARUCODE_API_KEY / KARUCODE_MODEL
  sudo systemctl restart ${SERVICE_NAME}

Upgrading later: re-run this script. The drop-in keeps your paths, and a running
service is restarted onto the new binary. Roll back with:
  sudo cp -p ${BIN_DIR}/karucode.old ${BIN_DIR}/karucode && sudo systemctl restart ${SERVICE_NAME}
EOF
    exit 0
fi

if [ "$DID_START" -eq 0 ]; then
    cat <<EOF
Service: left alone (--no-start/--no-restart). Start it with:

  sudo systemctl start ${SERVICE_NAME}
EOF
    if [ "$ENABLED_STATE" = "enabled" ]; then
        printf 'It is enabled, so it will also start on the next boot.\n'
    fi
    exit 0
fi

cat <<EOF
Service: NOT running.

Check why, in this order:

  systemctl status ${SERVICE_NAME} --no-pager -l
  journalctl -u ${SERVICE_NAME} -n 50 --no-pager

Most common causes:
  * port ${PORT_ARG:-8787} already in use by a hand-started karucode
        ss -ltnp | grep :${PORT_ARG:-8787}     # find the pid, then stop it
  * ${WORKSPACE} is not writable by ${SERVICE_USER}
        chown -R ${SERVICE_USER}:${SERVICE_GROUP} ${WORKSPACE}
  * ${DATA_DIR} is not writable by ${SERVICE_USER}
        chown -R ${SERVICE_USER}:${SERVICE_GROUP} ${DATA_DIR}
  * a bad value in ${ENV_FILE}, or KARUCODE_CONFIG pointing at a missing file
  * EPERM in the journal: the unit's SystemCallFilter is blocking a tool. Add a
    narrow SystemCallFilter=~@... exception in ${DROPIN_DIR}/20-local.conf
  * node/java crashing with SIGSEGV: you passed --no-jit; drop it

Files installed by this run:
  ${BIN_DIR}/karucode
  ${UNIT_PATH}
  ${DROPIN_PATH}
EOF
exit 2