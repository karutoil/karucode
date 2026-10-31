#!/bin/sh
# shellcheck disable=SC2317  # release_lock/report_status are only called from EXIT traps
# update.sh - rebuild karucode from this checkout and move the running systemd
# install onto the new binary.
#
#   sudo sh packaging/update.sh                 # fetch, build, stage, switch
#   sudo sh packaging/update.sh --dry-run       # print the plan, change nothing
#   sh packaging/update.sh --check              # is there anything to update?
#   sudo sh packaging/update.sh --stage-only    # build and validate; switch later
#   sudo sh packaging/update.sh --from-stage    # switch onto the staged build
#   sh packaging/update.sh --follow             # reattach to a detached run's log
#   sudo sh packaging/update.sh --no-pull       # build and install this tree
#   sudo sh packaging/update.sh --ref v0.2.0    # build a tag instead of a branch
#
# The first install is packaging/install.sh; this script is for every upgrade
# after it. It deliberately does *not* reimplement the systemd half: install.sh
# already keeps `karucode.old` for a rollback, rewrites the unit and the
# `10-install.conf` drop-in without losing the paths, clears a stale
# StartLimitBurst, restarts a running service and only reports success once
# `/api/health` answers. Two copies of that logic is how the two paths drift.
#
# What this script adds is the build, and the two things the installer cannot
# know:
#
#   1. It refuses to ship a dirty checkout, and fast-forwards the current branch
#      (or checks out --ref). What gets built is then what the repository says,
#      not what happened to be in the working tree.
#   2. It reads the *running* unit - ExecStart=, User=, WorkingDirectory=, the
#      KARUCODE_* Environment= lines, MemoryDenyWriteExecute= - and passes that
#      configuration to install.sh. The defaults compiled into install.sh are
#      for a fresh host (`/srv/karucode/workspace`, user `karucode`); an update
#      must keep the paths this service actually starts with. A workspace that
#      does not exist is a hard error, not a guess: the unit fails with
#      status=200/CHDIR if the drop-in ends up pointing somewhere else.
#   3. It builds as the checkout's owner, with the unit's own PATH. `sudo
#      ./update.sh` that compiles as root leaves root-owned files in `target/`
#      and `~/.cargo`, and the next build as yourself then fails on permissions.
#   4. It snapshots the SQLite database before anything restarts. There is no
#      automatic pre-migration snapshot, and an upgrade can move the schema.
#
# The run is split into a stage and a switch, and the whole thing is built to
# survive losing the terminal:
#
#   stage    fetch, build, copy the binary and the unit into a root-owned stage
#            directory, and validate *those copies* (they have to run, and the
#            unit has to parse). Nothing live is touched, so any failure here
#            leaves the running service exactly as it was. `--stage-only` stops
#            after this; `--from-stage` applies an existing stage without
#            rebuilding.
#   switch   snapshot the database, then install.sh moves the binary, rewrites
#            the drop-in and restarts the unit onto it. This is the only part
#            that can leave the service down, and it starts from artifacts that
#            have already been proven to work on this machine.
#
# A run that will build or switch detaches itself into its own session with its
# own log before doing anything (unless --no-detach): the build takes minutes,
# and a dropped SSH session during the switch is the one case where nobody can
# tell what state the install is in. The shell only waits and relays the exit
# code; `--follow` reattaches to the log from anywhere.
#
# Only the steps that need root are run under sudo: the stage, the snapshot and
# install.sh. The fetch and the build run as you, so you see an ordinary cargo
# build and the artifacts stay yours.
#
# Exit codes:
#   0  updated, and the service is serving the new binary
#   1  nothing was changed (bad argument, dirty tree, build failed, no unit)
#   2  installed, but the service did not come back - install.sh prints the
#      checklist, and `karucode.old` is the previous binary
#   3  --check only: the checkout is behind its upstream (nothing changed)

set -eu

# sudo and some minimal environments hand over a PATH without sbin, where
# runuser/install live. Append them rather than prepend: a user's cargo or node
# must still win over anything in /usr/local, exactly as in an interactive shell.
PATH="${PATH:-/usr/bin:/bin}:/usr/local/sbin:/usr/sbin:/sbin"
export PATH

SYSTEMCTL="${KARUCODE_SYSTEMCTL:-systemctl}"
SERVICE="${KARUCODE_SERVICE:-karucode}"

# Keep the arguments as given, before the option loop consumes them: a detached
# child re-executes this script and has to see the same command line.
ORIGINAL_ARGS=""
for _a in "$@"; do
    ORIGINAL_ARGS="${ORIGINAL_ARGS}${ORIGINAL_ARGS:+
}${_a}"
done
unset _a

# The detached child has to run with exactly the arguments this process was
# given. `$@` cannot be used for that - the option loop below consumes it - so
# the parent writes the original argv to a file and the child restores it here.
# One argument per line: an argument containing a newline is not supported, and
# nothing this script accepts is one.
if [ -n "${KARUCODE_ARGS_FILE:-}" ] && [ -f "${KARUCODE_ARGS_FILE}" ]; then
    # The file holds one argument per line (no argument this script accepts can
    # contain a newline). Reading it needs an explicit newline IFS: the default
    # IFS would also split on spaces inside a path, and globbing must stay off
    # so an argument containing * or ? is taken literally.
    _saved_args="$(cat "$KARUCODE_ARGS_FILE" 2>/dev/null || true)"
    rm -f "$KARUCODE_ARGS_FILE" 2>/dev/null || true
    unset KARUCODE_ARGS_FILE
    ORIGINAL_ARGS="$_saved_args"
    if [ -n "$_saved_args" ]; then
        set -f
        _old_ifs="$IFS"
        IFS='
'
        # shellcheck disable=SC2086  # deliberate: split the saved argv on newlines
        set -- $_saved_args
        IFS="$_old_ifs"
        set +f
        unset _old_ifs
    fi
    unset _saved_args
fi

# --- defaults ---------------------------------------------------------------

SOURCE=""
INSTALLER=""
CARGO_BIN="${KARUCODE_CARGO:-}"
BUILD_USER=""
WORKSPACE=""
DATA_DIR=""
HOST_ARG=""
PORT_ARG=""
PREFIX=""
SVC_USER=""
SVC_GROUP=""
SVC_ENV_FILE=""
REF=""
BACKUP_DIR=""
STAGE_DIR=""
UNIT_SRC_ARG=""

PULL=1
FETCH=1
CHECK=0
DRY_RUN=0
FORCE=0
ALLOW_DIRTY=0
BACKUP=1
SKIP_BUILD=0
BUILD_ONLY=0
NO_SUDO=0

# Staging: build and validate into a root-owned area first, then switch. The
# switch is the only step that touches the live binary, so a failed build or an
# invalid artifact cannot leave the service half-updated.
STAGE=1
STAGE_ONLY=0
FROM_STAGE=0

# Detaching: a build takes minutes and a dropped SSH session would otherwise
# kill it after it may already have moved the binary. -1 = decide later.
DETACH=-1
FOLLOW=0
JOB_DIR=""
LOCK_DIR=""
LOCKED=0

# --- output -----------------------------------------------------------------

die() {
    printf 'update.sh: error: %s\n' "$*" >&2
    exit 1
}

warn() {
    printf 'update.sh: warning: %s\n' "$*" >&2
}

info() {
    printf '==> %s\n' "$*"
}

usage() {
    cat <<'EOF'
Rebuild karucode from source and update the existing systemd install.

Usage: update.sh [OPTIONS]

Source:
  --source <DIR>        the checkout to build (default: the parent of packaging/)
  --installer <PATH>    run this script instead of packaging/install.sh
                        (the test suite uses it; you should not need it)
  --ref <REF>           build this tag/branch/commit instead of the current one
                        (leaves a detached HEAD; `git switch -` undoes it)
  --no-pull             build the working tree as it is; no fetch, no pull
  --no-fetch            with --check: do not contact the remote, use what is local
  --allow-dirty         permit modified tracked files (they get built and shipped)

Build:
  --skip-build          use the binary already in target/release instead of
                        compiling (what a re-run of a just-finished build does)
  --build-only          build and stop; do not touch the service
  --build-user <NAME>   compile as this user (default: the owner of --source)
  --cargo <PATH>        cargo to run (default: this user's, then the unit's PATH)

Install (each one overrides what the unit says):
  --workspace <DIR>     directory the agent may read and edit
  --data-dir <DIR>      SQLite database and plugin data
  --service-user <NAME> run the service as this user
  --service-group <NAME> ... and this group (default: same as the user)
  --host <ADDR>         bind address
  --port <PORT>         listen port
  --prefix <DIR>        install the binary to <DIR>/bin
  --env-file <PATH>     environment file handed to install.sh
  --no-backup           skip the pre-upgrade database snapshot
  --backup-dir <DIR>    where to write it (default: <data-dir>/backups)
  --no-sudo             never invoke sudo (for a root shell, and for tests)
  --force               install even when the binary and unit are unchanged

Staging and switching:
  --stage-only          build and validate everything, then stop before the
                        switch. Re-run with --from-stage to apply.
  --from-stage          switch onto what --stage-only staged; no rebuild,
                        no fetch. Refused if no valid stage exists.
  --stage-dir <DIR>     where staged artifacts live (default:
                        <data-dir>/stage); must be root-owned
  --no-stage            install straight from the build, skipping the stage
  --detach              run the whole update in a detached session so closing
                        or losing the terminal cannot stop it (default when
                        attached to a terminal and running elevated)
  --no-detach           stay in the foreground
  --follow              with --detach: attach to a detached run's log

Other:
  --check               report whether an update is available; change nothing
  --dry-run             print every action; build nothing, change nothing
  -h, --help            show this help

Exit codes: 0 updated and serving, 1 nothing changed, 2 installed but the
service did not come back, 3 --check found commits you do not have yet.
EOF
}

need_value() {
    [ $# -ge 2 ] || die "$1 needs a value (try --help)"
}

# --- argument parsing -------------------------------------------------------

while [ $# -gt 0 ]; do
    case "$1" in
        --source)          need_value "$@"; SOURCE="$2"; shift 2 ;;
        --source=*)        SOURCE="${1#*=}"; shift ;;
        --installer)       need_value "$@"; INSTALLER="$2"; shift 2 ;;
        --installer=*)     INSTALLER="${1#*=}"; shift ;;
        --ref)             need_value "$@"; REF="$2"; shift 2 ;;
        --ref=*)           REF="${1#*=}"; shift ;;
        --no-pull)         PULL=0; shift ;;
        --no-fetch)        FETCH=0; shift ;;
        --allow-dirty)     ALLOW_DIRTY=1; shift ;;
        --skip-build)      SKIP_BUILD=1; shift ;;
        --build-only)      BUILD_ONLY=1; shift ;;
        --build-user)      need_value "$@"; BUILD_USER="$2"; shift 2 ;;
        --build-user=*)    BUILD_USER="${1#*=}"; shift ;;
        --cargo)           need_value "$@"; CARGO_BIN="$2"; shift 2 ;;
        --cargo=*)         CARGO_BIN="${1#*=}"; shift ;;
        --workspace)       need_value "$@"; WORKSPACE="$2"; shift 2 ;;
        --workspace=*)     WORKSPACE="${1#*=}"; shift ;;
        --data-dir)        need_value "$@"; DATA_DIR="$2"; shift 2 ;;
        --data-dir=*)      DATA_DIR="${1#*=}"; shift ;;
        --service-user)    need_value "$@"; SVC_USER="$2"; shift 2 ;;
        --service-user=*)  SVC_USER="${1#*=}"; shift ;;
        --service-group)   need_value "$@"; SVC_GROUP="$2"; shift 2 ;;
        --service-group=*) SVC_GROUP="${1#*=}"; shift ;;
        --host)            need_value "$@"; HOST_ARG="$2"; shift 2 ;;
        --host=*)          HOST_ARG="${1#*=}"; shift ;;
        --port)            need_value "$@"; PORT_ARG="$2"; shift 2 ;;
        --port=*)          PORT_ARG="${1#*=}"; shift ;;
        --prefix)          need_value "$@"; PREFIX="$2"; shift 2 ;;
        --prefix=*)        PREFIX="${1#*=}"; shift ;;
        --env-file)        need_value "$@"; SVC_ENV_FILE="$2"; shift 2 ;;
        --env-file=*)      SVC_ENV_FILE="${1#*=}"; shift ;;
        --unit-file)       need_value "$@"; UNIT_SRC_ARG="$2"; shift 2 ;;
        --unit-file=*)     UNIT_SRC_ARG="${1#*=}"; shift ;;
        --backup-dir)      need_value "$@"; BACKUP_DIR="$2"; shift 2 ;;
        --backup-dir=*)    BACKUP_DIR="${1#*=}"; shift ;;
        --no-backup)       BACKUP=0; shift ;;
        --stage-only)      STAGE_ONLY=1; shift ;;
        --from-stage)      FROM_STAGE=1; shift ;;
        --stage-dir)       need_value "$@"; STAGE_DIR="$2"; shift 2 ;;
        --stage-dir=*)     STAGE_DIR="${1#*=}"; shift ;;
        --no-stage)        STAGE=0; shift ;;
        --detach)          DETACH=1; shift ;;
        --no-detach)       DETACH=0; shift ;;
        --follow)          FOLLOW=1; shift ;;
        --no-sudo)         NO_SUDO=1; shift ;;
        --force)           FORCE=1; shift ;;
        --check)           CHECK=1; shift ;;
        --dry-run)         DRY_RUN=1; shift ;;
        -h|--help)         usage; exit 0 ;;
        *)                 die "unknown argument: $1 (try --help)" ;;
    esac
done

if [ -n "$PORT_ARG" ]; then
    case "$PORT_ARG" in
        *[!0-9]*) die "--port must be a number, got: ${PORT_ARG}" ;;
    esac
    if [ "$PORT_ARG" -lt 1 ] || [ "$PORT_ARG" -gt 65535 ]; then
        die "--port must be between 1 and 65535, got: ${PORT_ARG}"
    fi
fi

if [ "$STAGE_ONLY" -eq 1 ] && [ "$FROM_STAGE" -eq 1 ]; then
    die "--stage-only and --from-stage are opposites; pick one"
fi
if [ "$FROM_STAGE" -eq 1 ] && [ "$STAGE" -eq 0 ]; then
    die "--from-stage needs the stage; --no-stage contradicts it"
fi
if [ "$FROM_STAGE" -eq 1 ] && { [ "$BUILD_ONLY" -eq 1 ] || [ "$CHECK" -eq 1 ]; }; then
    die "--from-stage applies a stage; it cannot be combined with --build-only or --check"
fi
if [ "$STAGE_ONLY" -eq 1 ] && [ "$BUILD_ONLY" -eq 1 ]; then
    die "--stage-only already stops before the switch; drop --build-only"
fi

# --- who we are, and who must do the privileged parts ------------------------

script_dir="$(unset CDPATH; cd -- "$(dirname -- "$0")" 2>/dev/null && pwd)" \
    || die "cannot resolve the directory of $0"

if [ -z "$SOURCE" ]; then
    SOURCE="$(dirname -- "$script_dir")"
fi
SOURCE="$(unset CDPATH; cd -- "$SOURCE" 2>/dev/null && pwd)" \
    || die "--source is not a directory: ${SOURCE}"
[ -f "${SOURCE}/Cargo.toml" ] \
    || die "${SOURCE} does not look like a karucode checkout (no Cargo.toml)"
if [ -z "$INSTALLER" ]; then
    INSTALLER="${SOURCE}/packaging/install.sh"
fi
# The existence check is deferred to just before the install: --check,
# --build-only and --dry-run have no business requiring it, and a test harness
# points --installer at a stub.

IS_ROOT=0
[ "$(id -u)" -eq 0 ] && IS_ROOT=1

# SUDO is empty when there is nothing to elevate (already root, or --no-sudo).
SUDO=""
if [ "$IS_ROOT" -eq 0 ] && [ "$NO_SUDO" -eq 0 ]; then
    command -v sudo >/dev/null 2>&1 \
        || die "installing needs root and sudo is not installed;
re-run as root, or pass --no-sudo along with --build-only/--check/--dry-run."
    SUDO="sudo"
fi

# Inside the karucode service itself (or any NoNewPrivileges unit) sudo cannot
# escalate, and the error it prints is about a flag, not about the cause. Say so
# here, while the user still remembers what they typed.
#
# The flag alone is not proof: the definitive test is whether sudo can actually
# raise privileges, so ask it (non-interactively, so nothing blocks) and only
# complain when it says no.
#
# --no-sudo is exempt: it means "do not elevate, run install.sh as I am". The
# installer then refuses with its own message if it really needs root, which is
# the right error for a root shell, a test harness, or a stubbed installer.
if [ "$IS_ROOT" -eq 0 ] && [ "$NO_SUDO" -eq 0 ] && [ "$DRY_RUN" -eq 0 ] \
    && [ "$CHECK" -eq 0 ] && [ "$BUILD_ONLY" -eq 0 ] && [ "$FOLLOW" -eq 0 ] \
    && grep -qs '^NoNewPrivs:[[:space:]]*1' /proc/self/status \
    && ! "$SUDO" -n true 2>/dev/null; then
    die "this shell has NoNewPrivileges set (it is running inside the karucode service), so sudo cannot
escalate. Run the update from a normal terminal on the host, not from an agent session."
fi

# --- running commands as the right user -------------------------------------

home_of() {
    _home="$(awk -F: -v u="$1" '$1 == u { print $6; exit }' /etc/passwd 2>/dev/null || true)"
    [ -n "$_home" ] || _home="/"
    printf '%s\n' "$_home"
}

# as_user USER CMD... - run a command so that files it creates stay owned by
# USER. HOME is set explicitly: `runuser` (unlike `sudo -u`) keeps root's
# environment, and cargo would then read /root/.cargo and write a toolchain of
# root-owned files into the user's target directory.
as_user() {
    _u="$1"
    shift
    if [ "$_u" = "$(id -un)" ]; then
        "$@"
    elif [ "$IS_ROOT" -eq 1 ] && command -v runuser >/dev/null 2>&1; then
        runuser -u "$_u" -- env HOME="$(home_of "$_u")" "$@"
    elif command -v sudo >/dev/null 2>&1; then
        sudo -n -u "$_u" -- env HOME="$(home_of "$_u")" "$@"
    else
        die "cannot run a command as ${_u}: neither runuser nor sudo is available"
    fi
}

# The same, but for a file in the data directory: root when the file needs root
# to read (root-owned data under /var/lib), the service user when root is what
# we already are, and ourselves when that is enough. Running sqlite3 as the
# service user for a database we own only makes it fail with "unable to open".
as_data_owner() {
    if [ "$SVC_USER" = "$(id -un)" ]; then
        "$@"
    elif [ "$IS_ROOT" -eq 1 ]; then
        as_user "$SVC_USER" "$@"
    else
        "$@"
    fi
}

# --- keeping a run exclusive and survivable ---------------------------------
#
# Two guarantees the script owes an operator:
#
#   1. One update at a time. Two concurrent runs would build into the same
#      target/, stage over the same directory and restart the unit twice; the
#      loser would then report a success it did not achieve.
#   2. Surviving the terminal. The build takes minutes and the switch takes
#      seconds. A dropped SSH session in the middle of the switch is the one
#      case where the operator cannot tell what state they are in, so the work
#      runs in its own session with its own log and the shell only waits.

# Running the whole update detached gives us a fresh process group and stdio.
KARUCODE_DETACHED="${KARUCODE_DETACHED:-0}"

# Should this run detach itself? Only when it has a terminal to detach *from*
# and something to protect: building, or switching. --check, --dry-run and
# --build-only are read-only in the sense that matters (nothing under /usr or
# /etc changes), and detaching them would only lose the exit code and the
# output the caller asked for.
decide_detach() {
    [ "$DETACH" -ge 0 ] && return 0
    DETACH=0
    [ "$KARUCODE_DETACHED" = "1" ] && return 0
    [ "$DRY_RUN" -eq 1 ] && return 0
    [ "$CHECK" -eq 1 ] && return 0
    [ "$STAGE_ONLY" -eq 1 ] && return 0
    [ "$BUILD_ONLY" -eq 1 ] && return 0
    # No tty means sudo cannot prompt for a password, so a detached run would
    # hang invisibly at the first privileged command. Only detach when the run
    # is already privileged, or when it does not need to elevate at all.
    [ -t 0 ] && [ -t 1 ] || return 0
    if [ "$IS_ROOT" -ne 1 ] && [ -z "$SUDO" ]; then
        return 0
    fi
    if [ -n "$SUDO" ]; then
        "$SUDO" -n true 2>/dev/null || return 0
    fi
    DETACH=1
}

take_lock() {
    _data="$(dirname -- "$(stage_dir_for)")"
    [ -n "$_data" ] || return 0
    # The lock lives next to the data dir, not inside it: the service user owns
    # that directory and the server holds a lock in it already.
    _parent="$(dirname -- "$_data")"
    LOCK_DIR="${_parent}/.$(basename -- "$_data").update.lock"

    # Try ourselves first, then escalated. A data dir we can write to needs no
    # root at all; a /var/lib one does.
    if mkdir "$LOCK_DIR" 2>/dev/null; then
        LOCKED=1
    elif [ -n "$SUDO" ] && "$SUDO" mkdir "$LOCK_DIR" 2>/dev/null; then
        LOCKED=1
    elif [ -e "$LOCK_DIR" ]; then
        # The directory exists, so this is real contention; name the holder.
        _owner="$(stat -c %U "$LOCK_DIR" 2>/dev/null || printf '?')"
        _pid="$(cat "${LOCK_DIR}/pid" 2>/dev/null || printf '?')"
        case "$_pid" in
            ''|*[!0-9]*)
                die "an update lock is held at ${LOCK_DIR} (owner ${_owner}) but it has no pid.
If no update is running, remove it:  sudo rm -rf ${LOCK_DIR}"
                ;;
        esac
        if ! kill -0 "$_pid" 2>/dev/null; then
            die "a stale update lock is held at ${LOCK_DIR} (pid ${_pid} is gone).
Remove it if no update is running:  sudo rm -rf ${LOCK_DIR}"
        fi
        die "another update is already running (pid ${_pid}, lock ${LOCK_DIR} owned by ${_owner}).
Wait for it to finish, or follow its log:  sh $0 --follow"
    elif [ -w "$_parent" ]; then
        # Writable parent but mkdir still failed: something odd, but not
        # contention, so do not claim another update is running.
        die "could not create the update lock at ${LOCK_DIR} (the parent is writable;
mkdir failed for another reason). Pass --no-sudo to run without it if you are root."
    else
        # A read-only parent means we cannot serialise runs here at all. Say so
        # once and continue: refusing outright would make the script unusable in
        # a sandbox that only allows writes to the data dir itself.
        warn "cannot create an update lock at ${LOCK_DIR} (${_parent} is not writable)."
        warn "Continuing without it: two updates running at once would not be detected."
        LOCK_DIR=""
        LOCKED=0
        return 0
    fi

    ( umask 077; printf '%s\n' "$$" > "${LOCK_DIR}/pid" ) 2>/dev/null \
        || { [ -n "$SUDO" ] && printf '%s\n' "$$" | "$SUDO" tee "${LOCK_DIR}/pid" >/dev/null 2>&1; } \
        || true
}

release_lock() {
    [ "$LOCKED" -eq 1 ] || return 0
    [ -n "$LOCK_DIR" ] || return 0
    rm -rf "$LOCK_DIR" 2>/dev/null \
        || { [ -n "$SUDO" ] && "$SUDO" rm -rf "$LOCK_DIR" 2>/dev/null; } \
        || true
    LOCKED=0
}

# The lock and the job directory belong to root: the script runs partly as the
# operator and partly elevated, and a file either of them must write cannot be
# owned by the other.
# The job directory belongs to root (the run is partly elevated), but the log
# inside it is the operator's own output and they must be able to read it: this
# script's whole failure story is "the reason is in the log". Root-owned at 0700
# meant `sudo sh packaging/update.sh` followed by a bare "see <log>" left the
# person who ran it unable to see *anything* - the parent directory 0700 keeps
# the data safe, so 0755 + 0644 files inside it leaks nothing.
root_mkdir() {
    if [ "$IS_ROOT" -eq 1 ]; then
        install -d -m 0755 "$1"
    elif [ -n "$SUDO" ]; then
        "$SUDO" install -d -m 0755 "$1"
    else
        mkdir -p "$1" 2>/dev/null && chmod 0755 "$1" 2>/dev/null
    fi
    # A directory created by an earlier (0)700 run would still hide the log.
    if [ -n "$SUDO" ]; then
        "$SUDO" chmod 0755 "$1" 2>/dev/null || true
    elif [ "$IS_ROOT" -eq 1 ]; then
        chmod 0755 "$1" 2>/dev/null || true
    fi
}

# Make a file in the job directory readable by whoever ran the update.
root_readable() {
    if [ -n "$SUDO" ]; then
        "$SUDO" chmod 0644 "$1" 2>/dev/null || true
    elif [ "$IS_ROOT" -eq 1 ]; then
        chmod 0644 "$1" 2>/dev/null || true
    fi
    return 0
}

root_write() {
    _path="$1"
    shift
    if [ -n "$SUDO" ]; then
        "$SUDO" tee "$_path" >/dev/null
    elif [ "$IS_ROOT" -eq 1 ]; then
        cat > "$_path"
    else
        cat > "$_path" 2>/dev/null || return 1
    fi
}

# Move this run into its own session. The child gets a copy of the work, not a
# second run: it re-executes the same script with the same arguments and
# KARUCODE_DETACHED=1, which is what stops decide_detach() from forking forever
# and lets the child report its real exit code back through $STATUS.
detach_run() {
    root_mkdir "$JOB_DIR" || die "cannot create the job directory ${JOB_DIR}"
    # A record of what was asked for, so --follow can name it later.
    ( printf 'dir=%s\n' "$JOB_DIR"; printf 'started=%s\n' "$(date '+%F %T')" ) \
        | root_write "${JOB_DIR}/job" || true

    # The child re-executes this script, so it needs the original arguments.
    # They arrive through a file rather than "$@": the option loop consumed
    # "$@", and this function is called with nothing left to forward.
    _args_file="${JOB_DIR}/args"
    : > "${_args_file}" 2>/dev/null || true
    printf '%s\n' "$ORIGINAL_ARGS" > "${_args_file}" 2>/dev/null \
        || printf '%s\n' "$ORIGINAL_ARGS" | root_write "$_args_file" || true

    # Exported *before* the spawn: the child reads it during startup to restore
    # its argument list, and the environment is captured at fork time.
    KARUCODE_ARGS_FILE="$_args_file"
    export KARUCODE_ARGS_FILE
    # Without this the child would take the same decision and fork again, and
    # again, each level truncating the previous level's log.
    KARUCODE_DETACHED=1
    export KARUCODE_DETACHED

    if command -v setsid >/dev/null 2>&1; then
        # setsid: new session, so a hangup on the controlling terminal is not
        # delivered here at all. The nested `sh` keeps the exit code.
        nohup setsid sh "$0" < /dev/null > "${JOB_DIR}/log" 2>&1 &
    else
        warn "setsid is not installed; the update continues in the background but"
        warn "a closing terminal may still deliver SIGHUP. Install util-linux."
        nohup sh "$0" < /dev/null >> "${JOB_DIR}/log" 2>&1 &
    fi
    _pid=$!

    # How the pid gets recorded differs by stage: this shell may hold the
    # process, or sudo may have replaced it.
    if [ -n "$SUDO" ]; then
        printf '%s\n' "$_pid" | root_write "${JOB_DIR}/pid" || true
    else
        printf '%s\n' "$_pid" > "${JOB_DIR}/pid" 2>/dev/null || true
    fi

    info "running detached so the update finishes even if this terminal goes away"
    info "job dir  ${JOB_DIR}"
    info "log      ${JOB_DIR}/log"
    info "status   ${JOB_DIR}/status"
    info "follow   sh $0 --follow"


    # The waiting shell must not be the weak link either: nohup here ignores
    # SIGHUP, and the `trap` keeps the wait readable while the child works.
    trap '' HUP
    _ticks=0
    while kill -0 "$_pid" 2>/dev/null; do
        _ticks=$((_ticks + 1))
        # Show progress without swallowing the log: the child owns that file.
        if [ "$_ticks" -eq 1 ] || [ $((_ticks % 30)) -eq 0 ]; then
            _last="$(tail -n 1 "${JOB_DIR}/log" 2>/dev/null || true)"
            [ -n "$_last" ] && printf '    ... %s\n' "$_last"
        fi
        sleep 1
    done
    trap - HUP

    _child=0
    [ -f "${JOB_DIR}/status" ] && _child="$(cat "${JOB_DIR}/status" 2>/dev/null || printf 0)"
    case "$_child" in
        ''|*[!0-9]*) _child=0 ;;
    esac

    # Now that the run is over, let the operator read its own output.
    root_readable "${JOB_DIR}/log"
    root_readable "${JOB_DIR}/status"
    root_readable "${JOB_DIR}/job"

    printf '\n'
    if [ "$_child" -eq 0 ]; then
        info "update finished"
    else
        # The reason has to be here, not only in a file. A detached run that
        # fails and prints "see <log>" is useless if the log cannot be read, or
        # if the operator is already looking at another window - so print the
        # diagnosis itself. The log keeps the full transcript for a post-mortem.
        printf 'update.sh: the detached run exited %s\n\n' "$_child" >&2
        _why="$(grep -E '^update\.sh: (error|warning):' "${JOB_DIR}/log" 2>/dev/null | tail -n 8 || true)"
        if [ -n "$_why" ]; then
            printf '%s\n' "$_why" | sed 's/^/  /' >&2
            printf '\n' >&2
        fi
        printf 'Full log: %s\n' "${JOB_DIR}/log" >&2
        if [ ! -r "${JOB_DIR}/log" ]; then
            printf '(that log is not readable as you; read it as root: sudo cat %s)\n' \
                "${JOB_DIR}/log" >&2
        fi
        printf 'Follow a rerun with: sh %s --follow\n' "$0" >&2
    fi
    exit "$_child"
}

follow_job() {
    _dir="$JOB_DIR"
    if [ -z "$_dir" ] || [ ! -d "$_dir" ]; then
        die "--follow needs a job; pass --stage-dir or run the update first"
    fi
    if [ ! -f "${_dir}/log" ]; then
        die "no log at ${_dir}/log (${_dir} is not a job directory)"
    fi
    # A run started with sudo writes its log as root. Say so instead of letting
    # tail fail with a bare "Permission denied" and no explanation.
    if [ ! -r "${_dir}/log" ]; then
        die "the log at ${_dir}/log is not readable as $(id -un).
It was written by an elevated run. Read it with:
    sudo cat ${_dir}/log
    sudo tail -f ${_dir}/log"
    fi
    if [ -f "${_dir}/status" ]; then
        info "job already finished (status $(cat "${_dir}/status")):"
        cat "${_dir}/log"
        return 0
    fi
    _pid="$(cat "${_dir}/pid" 2>/dev/null || true)"
    if [ -n "$_pid" ] && kill -0 "$_pid" 2>/dev/null; then
        info "following ${_dir}/log (pid ${_pid}); Ctrl-C stops following, not the job"
    else
        info "following ${_dir}/log"
    fi
    # Ctrl-C here must not kill the job: only this reader exits.
    trap 'exit 130' INT
    tail -f "${_dir}/log"
}


# --- read the running unit ---------------------------------------------------
# `systemctl show` is the merged view (unit + drop-ins + StateDirectory), which
# is what the service actually started with. Reading 10-install.conf instead
# would read the file, not the configuration.

unit_prop() {
    "$SYSTEMCTL" show "$SERVICE" -p "$1" 2>/dev/null \
        | sed -n "s/^$1=//p" | head -n 1 || true
}

load_state="$(unit_prop LoadState)"
case "$load_state" in
    loaded|masked) ;;
    *)
        die "no systemd unit named ${SERVICE} (LoadState=${load_state:-unknown}).
This script updates an existing install; run packaging/install.sh for the first one."
        ;;
esac

exec_line="$(unit_prop ExecStart)"
exec_path="$(printf '%s\n' "$exec_line" | sed -n 's/.*path=\([^ ;]*\).*/\1/p')"
[ -n "$exec_path" ] \
    || die "cannot read ExecStart= from ${SERVICE}.service (got: ${exec_line:-nothing})"

# Where systemd actually loads the unit from - not a hardcoded
# /etc/systemd/system, which is wrong under --prefix or a custom unit dir.
UNIT_PATH="$(unit_prop FragmentPath)"
[ -n "$UNIT_PATH" ] || UNIT_PATH="/etc/systemd/system/${SERVICE}.service"

UNIT_ENVIRONMENT="$(unit_prop Environment)"
unit_env() {
    printf '%s\n' "$UNIT_ENVIRONMENT" | tr ' ' '\n' | sed -n "s/^$1=//p" | head -n 1 || true
}

if [ -z "$PREFIX" ]; then
    _bin_dir="${exec_path%/*}"
    PREFIX="${_bin_dir%/*}"
    if [ -z "$PREFIX" ] || [ "$PREFIX" = "$_bin_dir" ]; then
        die "cannot derive --prefix from ExecStart=${exec_path}; pass --prefix"
    fi
fi
[ -n "$SVC_USER" ] || SVC_USER="$(unit_prop User)"
[ -n "$SVC_GROUP" ] || SVC_GROUP="$(unit_prop Group)"
[ -n "$WORKSPACE" ] || WORKSPACE="$(unit_prop WorkingDirectory)"
[ -n "$DATA_DIR" ] || DATA_DIR="$(unit_env KARUCODE_DATA_DIR)"
[ -n "$HOST_ARG" ] || HOST_ARG="$(unit_env KARUCODE_HOST)"
[ -n "$PORT_ARG" ] || PORT_ARG="$(unit_env KARUCODE_PORT)"

if [ -z "$DATA_DIR" ]; then
    _state="$(unit_prop StateDirectory)"
    if [ -n "$_state" ]; then
        DATA_DIR="/var/lib/${_state%% *}"
    fi
fi
[ -n "$DATA_DIR" ] || DATA_DIR="/var/lib/karucode"
# No Group= fallback to the user name: that assumption is exactly what this
# forwarding exists to avoid, and it is not a value the running unit ever held.

ALLOW_JIT=0
[ "$(unit_prop MemoryDenyWriteExecute)" = "no" ] && ALLOW_JIT=1

# A miss here means the unit's environment could not be parsed (a value with a
# space is printed quoted by systemd, and the split above would truncate it).
# Failing loudly beats handing install.sh a wrong path that becomes the new
# drop-in - that is the failure mode that leaves a service unable to start.
[ -n "$WORKSPACE" ] \
    || die "cannot determine the workspace from ${SERVICE}.service; pass --workspace"
[ -d "$WORKSPACE" ] \
    || die "workspace ${WORKSPACE} does not exist; pass --workspace (the unit would fail with status=200/CHDIR)"
[ -d "$DATA_DIR" ] \
    || die "data dir ${DATA_DIR} does not exist; pass --data-dir"
[ -n "$SVC_USER" ] \
    || die "the unit does not set User=; pass --service-user (install.sh refuses to run a unit as root)"
if [ "$SVC_USER" = "root" ]; then
    die "the unit runs as root; this updater refuses to keep that. Pass --service-user <name>."
fi
# The group is forwarded, not assumed. install.sh derives Group= from the account's
# real primary gid when it is not told otherwise, which is usually right - but "the
# group the running unit actually uses" is the only value that cannot change what
# the service is, so it is passed explicitly.
[ -n "$SVC_GROUP" ] \
    || die "the unit does not set Group=; pass --service-group (install.sh would use ${SVC_USER}'s primary group)"

# --- staging: prove the artifacts before touching anything live -------------
#
# The switch is the only step that touches the live binary and the unit, and it
# is the only step that can leave the service down. So everything that can fail
# - the binary running, the unit parsing as a unit, the staged pair matching -
# is checked while the service is still serving the old binary. The stage also
# carries the artifacts across a privilege boundary: the build runs as the
# operator, while install.sh has to read the files as root, and a checkout the
# operator cannot read (or a data dir root cannot traverse) must not decide
# whether the update works.
#
# The stage is root-owned and world-readable for that reason: written once by
# whoever ran the build, then only read.

stage_dir_for() {
    if [ -n "$STAGE_DIR" ]; then
        printf '%s\n' "$STAGE_DIR"
    else
        printf '%s\n' "${DATA_DIR}/stage"
    fi
}

# Where the detached run's log, pid and exit code live. Always beside the data
# dir, so --follow can find it without repeating --detach's other flags.
job_dir_for() {
    if [ -n "$JOB_DIR" ]; then
        printf '%s\n' "$JOB_DIR"
    else
        printf '%s\n' "${DATA_DIR}/update-job"
    fi
}

stage_binary() { printf '%s\n' "$(stage_dir_for)/karucode"; }
stage_unit()   { printf '%s\n' "$(stage_dir_for)/${SERVICE}.service"; }
stage_info()   { printf '%s\n' "$(stage_dir_for)/STAGE"; }

# Read one key from the stage manifest. `key=value`, one per line.
stage_field() {
    _file="$(stage_info)"
    [ -f "$_file" ] || return 0
    sed -n "s/^$1=//p" "$_file" 2>/dev/null | head -n 1
}

# The unit file a switch would install: the checkout's copy when there is one,
# else whatever is already installed (install.sh fetches from the release when
# given neither, which an offline update must not depend on).
unit_source_for_stage() {
    if [ -n "$UNIT_SRC_ARG" ]; then
        printf '%s\n' "$UNIT_SRC_ARG"
    elif [ -f "${SOURCE}/packaging/${SERVICE}.service" ]; then
        printf '%s\n' "${SOURCE}/packaging/${SERVICE}.service"
    elif [ -f "${UNIT_PATH}" ]; then
        printf '%s\n' "$UNIT_PATH"
    fi
}

# Write the stage. Everything that can be rejected is rejected here, while the
# running service is untouched.
do_stage() {
    _dir="$(stage_dir_for)"
    _bin="$(stage_binary)"
    _unit="$(stage_unit)"
    _src_bin="$1"

    [ -f "$_src_bin" ] || {
        if [ "$DRY_RUN" -eq 1 ]; then
            warn "no ${_src_bin} yet; the preview below describes a stage of a binary that does not exist"
        else
            die "cannot stage: ${_src_bin} does not exist"
        fi
    }
    _unit_src="$(unit_source_for_stage)"

    # A dry run has no artifact to validate yet - that is the whole point of it
    # - so it describes the stage instead of performing it.
    if [ "$DRY_RUN" -eq 1 ]; then
        info "would stage ${_src_bin} -> ${_bin}"
        if [ -n "$_unit_src" ]; then
            info "would stage ${_unit_src} -> ${_unit}"
        else
            warn "no unit file found (${SOURCE}/packaging/${SERVICE}.service or ${UNIT_PATH})"
        fi
        info "would validate the staged pair (run --version, parse the unit) and record a sha256"
        return 0
    fi

    [ -n "$_unit_src" ] || die "cannot stage: no unit file to install
(looked for ${SOURCE}/packaging/${SERVICE}.service and ${UNIT_PATH}; pass --unit-file)"

    info "staging into ${_dir}"
    # A stale stage must never be applied by a later --from-stage, so clear it
    # first: the manifest is what makes a stage valid, and it is written last.
    if [ -d "$_dir" ]; then
        rm -rf "$_dir" 2>/dev/null || { [ -n "$SUDO" ] && "$SUDO" rm -rf "$_dir" 2>/dev/null; } \
            || die "could not clear the previous stage at ${_dir}"
    fi
    root_mkdir "$_dir" || die "could not create the stage directory ${_dir}"
    if [ -n "$SUDO" ]; then
        "$SUDO" chmod 0755 "$_dir"
    elif [ "$IS_ROOT" -eq 1 ]; then
        chmod 0755 "$_dir"
    fi

    # install(1), not cp: the file lands with its mode in one step, and a
    # half-written binary can never be picked up as staged. Ownership is only
    # asserted when we really are root; `sudo install -o root` as a non-root
    # user inside a stub fails for a reason that has nothing to do with staging.
    if [ "$IS_ROOT" -eq 1 ]; then
        install -m 0755 -o root -g root "$_src_bin" "${_bin}.partial" \
            || die "could not write ${_bin}.partial"
        install -m 0644 -o root -g root "$_unit_src" "${_unit}.partial" \
            || die "could not write ${_unit}.partial"
    elif [ -n "$SUDO" ]; then
        "$SUDO" install -m 0755 "$_src_bin" "${_bin}.partial" \
            || die "could not write ${_bin}.partial"
        "$SUDO" install -m 0644 "$_unit_src" "${_unit}.partial" \
            || die "could not write ${_unit}.partial"
    else
        install -m 0755 "$_src_bin" "${_bin}.partial" || die "could not write ${_bin}.partial"
        install -m 0644 "$_unit_src" "${_unit}.partial" || die "could not write ${_unit}.partial"
    fi

    # Validate the copies, not the originals: these are the bytes that will be
    # installed, so these are the bytes that must work.
    if ! "${_bin}.partial" --version >/dev/null 2>&1; then
        rm -f "${_bin}.partial" "${_unit}.partial" 2>/dev/null || true
        die "the staged binary does not run on this machine (wrong architecture or missing libc).
Nothing was switched: the service is still serving the previous binary."
    fi
    grep -q '^\[Unit\]' "${_unit}.partial" 2>/dev/null || {
        rm -f "${_bin}.partial" "${_unit}.partial" 2>/dev/null || true
        die "${_unit_src} is not a systemd unit (no [Unit] section). Nothing was switched."
    }

    # Promote both at once: two renames are as close to atomic as a filesystem
    # without a transaction gets, and the service is not involved yet.
    mv_into_place "${_bin}.partial" "$_bin"
    mv_into_place "${_unit}.partial" "$_unit"

    _sha="$(sha256sum "$_bin" | cut -d' ' -f1)"
    _ver="$("$_bin" --version 2>/dev/null || true)"
    {
        printf 'version=%s\n' "${_ver:-none}"
        printf 'sha256=%s\n' "${_sha}"
        printf 'source=%s\n' "$SOURCE"
        printf 'source_binary=%s\n' "$_src_bin"
        printf 'unit_source=%s\n' "$_unit_src"
        printf 'staged_at=%s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
        printf 'staged_by=%s\n' "$(id -un)"
    } | root_write "$(stage_info)" || die "could not write the stage manifest"

    info "staged ${_ver:-<no version>} (sha256 ${_sha})"
    info "staged the unit from ${_unit_src}"
}

mv_into_place() {
    if [ -n "$SUDO" ]; then
        "$SUDO" mv -f "$1" "$2"
    else
        mv -f "$1" "$2"
    fi
}

# Apply what is staged. This is the whole switch: one validated binary and one
# validated unit, handed to the installer that already knows how to swap them
# without losing the paths.
do_switch() {
    _dir="$(stage_dir_for)"
    _bin="$(stage_binary)"
    _unit="$(stage_unit)"

    [ -f "$_bin" ] || die "nothing staged at ${_bin}; run --stage-only first (or drop --from-stage)"
    [ -f "$_unit" ] || die "nothing staged at ${_unit}; run --stage-only first (or drop --from-stage)"
    [ -f "$(stage_info)" ] || die "the stage in ${_dir} is incomplete (no STAGE manifest).
A partially-written stage is never applied; re-run --stage-only."

    _staged_sha="$(stage_field sha256)"
    [ -n "$_staged_sha" ] || die "the stage manifest in ${_dir} has no sha256; re-run --stage-only"
    _actual_sha="$(sha256sum "$_bin" | cut -d' ' -f1)"
    [ "$_staged_sha" = "$_actual_sha" ] || die "the staged binary changed since it was validated
(expected ${_staged_sha}, found ${_actual_sha}). Re-run --stage-only."
    _staged_ver="$(stage_field version)"

    info "switching to the staged ${_staged_ver:-binary} (sha256 ${_actual_sha})"
    info "  binary ${_bin}"
    info "  unit   ${_unit}"
}

# --- the plan ---------------------------------------------------------------

if [ -n "$BUILD_USER" ]; then
    :
elif [ -n "$(stat -c %U "$SOURCE" 2>/dev/null || true)" ]; then
    BUILD_USER="$(stat -c %U "$SOURCE")"
else
    BUILD_USER="$(id -un)"
fi
if [ "$BUILD_USER" = "root" ] && [ "$IS_ROOT" -eq 1 ]; then
    warn "building as root: target/ and the cargo cache will be owned by root."
    warn "Pass --build-user <name> so the artifacts stay yours."
fi

installed_bin="${PREFIX}/bin/$(basename "$exec_path")"

info "source ${SOURCE} (build user: ${BUILD_USER})"
info "service ${SERVICE}: ${exec_path} running as ${SVC_USER}:${SVC_GROUP}"
info "workspace ${WORKSPACE}"
info "data dir  ${DATA_DIR}"
info "listen    ${HOST_ARG:-<unit default>}:${PORT_ARG:-<unit default>}"
if [ "$ALLOW_JIT" -eq 1 ]; then
    info "MemoryDenyWriteExecute=no in the unit: keeping --allow-jit"
fi

# --- become survivable and exclusive, before any work is done ---------------
# Both decisions are made before the first irreversible step: a lock taken after
# a ten-minute build would not stop a second build, and a detach after the
# switch would protect the wrong half of the run.

JOB_DIR="$(job_dir_for)"

if [ "$FOLLOW" -eq 1 ]; then
    follow_job
    exit 0
fi

decide_detach
if [ "$DETACH" -eq 1 ] && [ "$KARUCODE_DETACHED" != "1" ]; then
    # --follow and --check have no work to protect; everything else does.
    if [ "$CHECK" -eq 0 ] && [ "$DRY_RUN" -eq 0 ]; then
        detach_run "$@"
    fi
fi

if [ "$CHECK" -eq 0 ] && [ "$DRY_RUN" -eq 0 ]; then
    take_lock
    trap 'release_lock' EXIT INT TERM HUP
fi

# The child of a detached run reports its real exit code, which is what the
# waiting shell reads and what a cron job or a second terminal can check.
if [ "$KARUCODE_DETACHED" = "1" ] && [ -n "$JOB_DIR" ]; then
    report_status() {
        _code=$?
        printf '%s\n' "$_code" | root_write "${JOB_DIR}/status" 2>/dev/null || true
        release_lock
        exit "$_code"
    }
    trap 'report_status' EXIT
fi


# --- resolve cargo ----------------------------------------------------------
# When we are root and dropping to another user, that user's PATH is not ours to
# guess - but the unit already carries the PATH the service runs with, which is
# where that user's cargo lives (the agent has to be able to run cargo too).
cargo_from_path() {
    _p="$1"
    while [ -n "$_p" ]; do
        _d="${_p%%:*}"
        case "$_p" in
            *:*) _p="${_p#*:}" ;;
            *)   _p="" ;;
        esac
        if [ -n "$_d" ] && [ -x "${_d}/cargo" ]; then
            printf '%s\n' "${_d}/cargo"
            return 0
        fi
    done
    return 1
}

resolve_cargo() {
    [ -n "$CARGO_BIN" ] && return 0
    if [ "$BUILD_USER" != "$(id -un)" ]; then
        _unit_path="$(unit_env PATH)"
        if [ -n "$_unit_path" ] && CARGO_BIN="$(cargo_from_path "$_unit_path")"; then
            return 0
        fi
        CARGO_BIN=""
    fi
    if CARGO_BIN="$(command -v cargo 2>/dev/null)"; then
        [ -n "$CARGO_BIN" ] && return 0
    fi
    CARGO_BIN=""
    if _unit_path="$(unit_env PATH)" && [ -n "$_unit_path" ] \
        && CARGO_BIN="$(cargo_from_path "$_unit_path")"; then
        return 0
    fi
    CARGO_BIN=""
    die "cargo not found (looked in \$PATH and in the unit's PATH); pass --cargo /path/to/cargo"
}

# --- git: what are we building? ---------------------------------------------

cd "$SOURCE"
IS_REPO=0
[ -d "${SOURCE}/.git" ] && IS_REPO=1

git_() {
    as_user "$BUILD_USER" git "$@"
}

if [ "$IS_REPO" -eq 1 ]; then
    branch="$(git_ rev-parse --abbrev-ref HEAD 2>/dev/null || true)"
    head="$(git_ rev-parse --short HEAD 2>/dev/null || true)"

    # --from-stage applies bytes that are already staged and validated. Which
    # commit the checkout is on, and whether it is dirty, cannot change them, so
    # asking about it would only block a switch for an irrelevant reason.
    dirty_count="$(git_ status --porcelain --untracked-files=no 2>/dev/null | wc -l | tr -d ' ')"
    # --check only compares HEAD with its upstream; a dirty tree does not stop
    # that question from having an answer, so the refusal lives further down.
    if [ "$CHECK" -eq 0 ] && [ "$FROM_STAGE" -eq 0 ]; then
        if [ "$dirty_count" != "0" ] && [ "$ALLOW_DIRTY" -eq 0 ]; then
            printf 'update.sh: error: %s tracked file(s) are modified in %s:\n' "$dirty_count" "$SOURCE" >&2
            git_ status --short --untracked-files=no | sed 's/^/    /' >&2
            die "commit or stash them, or pass --allow-dirty to build and ship them anyway"
        fi
        if [ "$dirty_count" != "0" ]; then
            warn "building a dirty checkout on purpose (--allow-dirty)"
        fi
    fi

    if [ "$FETCH" -eq 1 ] && [ "$FROM_STAGE" -eq 0 ] \
        && { [ "$PULL" -eq 1 ] || [ "$CHECK" -eq 1 ]; }; then
        if [ "$DRY_RUN" -eq 1 ]; then
            info "would run: git fetch --prune"
        else
            info "git fetch --prune"
            git_ fetch --prune --quiet || die "git fetch failed (no network, or a credential prompt); use --no-pull to build what you have"
        fi
    fi

    if [ -n "$REF" ]; then
        if [ "$DRY_RUN" -eq 0 ]; then
            if ! git_ rev-parse --verify --quiet "$REF" >/dev/null 2>&1; then
                git_ fetch --tags --quiet || die "git fetch --tags failed for --ref ${REF}"
            fi
            info "checking out ${REF}"
            git_ checkout --quiet "$REF" || die "git checkout ${REF} failed"
            warn "HEAD is now detached at ${REF}; 'git switch -' returns to ${branch:-the previous branch}"
            head="$(git_ rev-parse --short HEAD)"
        else
            info "would check out ${REF}"
        fi
    elif [ "$PULL" -eq 1 ] && [ "$CHECK" -eq 0 ] && [ "$FROM_STAGE" -eq 0 ] \
        && [ -n "$branch" ] && [ "$branch" != "HEAD" ]; then
        upstream="$(git_ rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' 2>/dev/null || true)"
        if [ -z "$upstream" ]; then
            warn "${branch} has no upstream; skipping the pull"
        elif [ "$DRY_RUN" -eq 1 ]; then
            info "would run: git pull --ff-only (${upstream})"
        else
            before="$(git_ rev-parse HEAD)"
            info "git pull --ff-only ${upstream}"
            git_ pull --ff-only --quiet || die "git pull --ff-only failed; resolve it by hand, or use --no-pull"
            after="$(git_ rev-parse HEAD)"
            if [ "$before" = "$after" ]; then
                info "already at the tip of ${upstream}"
            else
                info "$(printf '%s' "$before" | cut -c1-7) -> $(printf '%s' "$after" | cut -c1-7) from ${upstream}"
            fi
        fi
    fi

    if [ "$CHECK" -eq 0 ] && [ "$FROM_STAGE" -eq 0 ]; then
        info "building ${branch:-HEAD}${head:+ (${head})}"
    fi
elif [ "$FROM_STAGE" -eq 0 ]; then
    warn "${SOURCE} is not a git checkout; building the tree as it is"
fi

# --- --check: is there anything to do? --------------------------------------

if [ "$CHECK" -eq 1 ]; then
    if [ "$IS_REPO" -ne 1 ]; then
        info "not a git checkout, so there is no upstream to compare against"
        exit 0
    fi
    upstream="$(git_ rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' 2>/dev/null || true)"
    if [ -z "$upstream" ]; then
        info "no upstream for ${branch:-HEAD}; nothing to compare"
        exit 0
    fi
    behind="$(git_ rev-list --count "HEAD..${upstream}" 2>/dev/null || printf '0')"
    if [ "$behind" = "0" ]; then
        info "up to date with ${upstream}"
        exit 0
    fi
    printf '\n%s commit(s) behind %s:\n\n' "$behind" "$upstream"
    git_ --no-pager log --oneline "HEAD..${upstream}" | head -n 20 | sed 's/^/    /'
    printf '\nRun this to build and install them:\n\n    sudo sh %s\n' "$0"
    exit 3
fi

# --- build ------------------------------------------------------------------

BUILT="${SOURCE}/target/release/karucode"

if [ "$FROM_STAGE" -eq 1 ]; then
    # --from-stage applies what was already built and validated. Compiling here
    # would defeat the point: the operator staged during a maintenance window,
    # and this is the switch. Assert the stage instead of trusting the message
    # --stage-only printed.
    [ -f "$(stage_binary)" ] || die "nothing staged at $(stage_binary)"
    info "--from-stage: using the stage in $(stage_dir_for), not building"
elif [ "$SKIP_BUILD" -eq 1 ]; then
    info "--skip-build: using the existing ${BUILT}"
    [ -f "$BUILT" ] || die "--skip-build was given but ${BUILT} does not exist"
elif [ "$DRY_RUN" -eq 1 ]; then
    resolve_cargo
    info "would run: ${CARGO_BIN} build --release -p kc-server (as ${BUILD_USER})"
    if [ ! -f "$BUILT" ]; then
        warn "no ${BUILT} yet, so the install preview below describes a binary that does not exist"
    fi
else
    resolve_cargo
    info "cargo build --release -p kc-server (as ${BUILD_USER}, ${CARGO_BIN})"
    info "the frontend is rebuilt and embedded by crates/kc-server/build.rs; it needs npm on PATH"
    # A failed build must leave the running service exactly as it was: nothing
    # below this point has touched /usr/local.
    if ! as_user "$BUILD_USER" "$CARGO_BIN" build --release -p kc-server; then
        die "cargo build failed; nothing was installed and the running binary is untouched"
    fi
fi

if [ "$SKIP_BUILD" -eq 0 ] && [ "$DRY_RUN" -eq 0 ] && [ ! -f "$BUILT" ]; then
    die "the build reported success but ${BUILT} is missing"
fi

if [ "$DRY_RUN" -eq 0 ] && [ -f "$BUILT" ]; then
    built_version="$("$BUILT" --version 2>/dev/null || true)"
    info "built: ${built_version:-<no --version output>} ($(wc -c < "$BUILT" | tr -d ' ') bytes)"
fi

if [ "$BUILD_ONLY" -eq 1 ]; then
    info "--build-only: stopping before the service is touched"
    exit 0
fi

# --- stage, then switch -----------------------------------------------------
#
# Two separate steps on purpose. The first can fail for a dozen boring reasons
# (wrong libc, a truncated file, a checkout root cannot read) and leaves the
# running service completely alone. Only the second moves the binary, rewrites
# the drop-in and restarts the unit.

[ -f "$INSTALLER" ] || die "no installer at ${INSTALLER} (pass --installer <path>)"

STAGED_BIN=""
STAGED_UNIT=""
if [ "$STAGE" -eq 1 ]; then
    if [ "$FROM_STAGE" -eq 0 ]; then
        do_stage "$BUILT"
    fi
    if [ "$STAGE_ONLY" -eq 1 ]; then
        if [ "$DRY_RUN" -eq 1 ]; then
            info "--stage-only: would stop here, before the switch"
            exit 0
        fi
        # Re-verify what is on disk, so the message names a stage that is
        # actually applyable rather than one that merely was written.
        do_switch >/dev/null 2>&1 || die "the stage in $(stage_dir_for) did not verify"
        cat <<EOF

Staged and verified, and nothing live has changed: the service is still serving
${installed_bin}.

Apply it with:

    sudo sh $0 --from-stage

Or throw it away with:

    sudo rm -rf $(stage_dir_for)
EOF
        exit 0
    fi
    STAGED_BIN="$(stage_binary)"
    STAGED_UNIT="$(stage_unit)"
    if [ "$DRY_RUN" -eq 1 ]; then
        # Nothing was staged, so preview with the real inputs instead: the build
        # that would be staged, and the unit source it would be copied from.
        # install.sh refuses a path that does not exist even when previewing,
        # which is the correct behaviour for a real run.
        STAGED_BIN="$BUILT"
        STAGED_UNIT="$(unit_source_for_stage)"
    fi
else
    STAGED_BIN="$BUILT"
    STAGED_UNIT="$(unit_source_for_stage)"
fi

# --- is it worth restarting the service at all? -----------------------------
# An unchanged binary and an unchanged unit mean the restart would only drop
# in-flight sessions. Compare both: a release can change the unit alone.

if [ "$FROM_STAGE" -eq 1 ]; then
    # The staged bytes are what will be installed, so they are what the
    # comparison has to use; BUILT may be an older build or absent entirely.
    CMP_BIN="$(stage_binary)"
else
    CMP_BIN="$BUILT"
fi

if [ -f "$CMP_BIN" ] && [ -f "$installed_bin" ] && [ "$FORCE" -eq 0 ] && [ "$DRY_RUN" -eq 0 ]; then
    built_sum="$(sha256sum "$CMP_BIN" | cut -d' ' -f1)"
    installed_sum="$(sha256sum "$installed_bin" | cut -d' ' -f1)"
    unit_changed=0
    if [ -n "$STAGED_UNIT" ] && [ -f "$STAGED_UNIT" ] && [ -f "${UNIT_PATH}" ]; then
        cmp -s "$STAGED_UNIT" "${UNIT_PATH}" || unit_changed=1
    elif [ -f "${SOURCE}/packaging/${SERVICE}.service" ] && [ -f "${UNIT_PATH}" ]; then
        cmp -s "${SOURCE}/packaging/${SERVICE}.service" "${UNIT_PATH}" || unit_changed=1
    else
        # No copy to compare against (a hand-installed unit, or an unusual
        # path): assume it changed and let install.sh decide.
        unit_changed=1
    fi
    if [ "$built_sum" = "$installed_sum" ] && [ "$unit_changed" -eq 0 ]; then
        info "already up to date: ${installed_bin} is this build and the unit is unchanged"
        printf '\nNothing was restarted. Use --force to reinstall anyway.\n'
        exit 0
    fi
    if [ "$built_sum" = "$installed_sum" ]; then
        info "the binary is unchanged but the unit file changed; reinstalling the unit"
    fi
fi

# --- pre-upgrade database snapshot ------------------------------------------
# The database holds sessions, settings and the provider key, and a new release
# may migrate the schema. `VACUUM INTO` is the online-snapshot form: it works
# while the server is running, and the result is a single file with no -wal.

db_path="${DATA_DIR}/karucode.db"
if [ "$BACKUP" -eq 0 ]; then
    info "skipping the database snapshot (--no-backup)"
elif [ ! -f "$db_path" ]; then
    warn "no database at ${db_path}; nothing to snapshot"
else
    sqlite_bin="$(command -v sqlite3 2>/dev/null || true)"
    if [ -z "$sqlite_bin" ]; then
        warn "sqlite3 is not installed, so the database was NOT snapshotted."
        warn "Back it up by hand before a schema migration, or install sqlite3."
    else
        if [ -z "$BACKUP_DIR" ]; then
            BACKUP_DIR="${DATA_DIR}/backups"
        fi
        stamp="$(date +%Y%m%d-%H%M%S)"
        snapshot="${BACKUP_DIR}/pre-update-${stamp}.db"
        if [ "$DRY_RUN" -eq 1 ]; then
            info "would snapshot ${db_path} to ${snapshot}"
        else
            info "snapshotting ${db_path} to ${snapshot}"
            # Whose job is it to create the backup dir? Root hands it to the
            # service user; anyone else may just create it themselves when they
            # own the data dir, and only reaches for sudo when they do not.
            if [ ! -d "$BACKUP_DIR" ]; then
                if [ "$IS_ROOT" -eq 1 ]; then
                    install -d -m 0700 -o "$SVC_USER" -g "$SVC_GROUP" "$BACKUP_DIR" \
                        || die "could not create ${BACKUP_DIR}"
                elif ! mkdir -p "$BACKUP_DIR" 2>/dev/null; then
                    die "could not create ${BACKUP_DIR}; pass --backup-dir to put the
snapshot somewhere you can write, or --no-backup to skip it."
                fi
                chmod 0700 "$BACKUP_DIR" 2>/dev/null || true
            fi
            escaped="$(printf '%s' "$snapshot" | sed "s/'/''/g")"
            if ! as_data_owner "$sqlite_bin" "$db_path" "VACUUM INTO '${escaped}'"; then
                die "the database snapshot failed (${snapshot}); nothing was installed.
Pass --no-backup to update without one, but know that a schema migration would
then have no copy to fall back to."
            fi
            info "snapshot written: ${snapshot}"
        fi
    fi
fi

# --- the switch -------------------------------------------------------------
# Everything below changes the running system. The build, the fetch, the staging
# and the snapshot have all succeeded by now, so a failure here is a genuine
# install problem and install.sh's exit codes are the ones that matter.

set -- \
    --binary "$STAGED_BIN" \
    --prefix "$PREFIX" \
    --workspace "$WORKSPACE" \
    --data-dir "$DATA_DIR" \
    --service-user "$SVC_USER" \
    --service-group "$SVC_GROUP"
if [ -n "$STAGED_UNIT" ]; then set -- "$@" --unit-file "$STAGED_UNIT"; fi
if [ -n "$HOST_ARG" ]; then set -- "$@" --host "$HOST_ARG"; fi
if [ -n "$PORT_ARG" ]; then set -- "$@" --port "$PORT_ARG"; fi
if [ -n "$SVC_ENV_FILE" ]; then set -- "$@" --env-file "$SVC_ENV_FILE"; fi
if [ "$ALLOW_JIT" -eq 1 ]; then set -- "$@" --allow-jit; fi
if [ "$DRY_RUN" -eq 1 ]; then set -- "$@" --dry-run; fi

if [ "$DRY_RUN" -eq 1 ]; then
    # install.sh accepts --dry-run without root and mutates nothing, so the
    # preview here is the real argument set, not a description of it.
    info "would run: sh ${INSTALLER} $*"
    exec sh "$INSTALLER" "$@"
fi

if [ "$FROM_STAGE" -eq 1 ]; then
    do_switch
else
    info "switching: installing the validated binary and unit via ${INSTALLER}"
fi

set +e
if [ -n "$SUDO" ]; then
    "$SUDO" sh "$INSTALLER" "$@"
    rc=$?
else
    sh "$INSTALLER" "$@"
    rc=$?
fi
set -e

case "$rc" in
    0) ;;
    2)
        printf '\nupdate.sh: the new binary is installed, but the service did not stay up.\n' >&2
        printf 'Roll back with:\n    sudo cp -p %s.old %s && sudo systemctl restart %s\n' \
            "$installed_bin" "$installed_bin" "$SERVICE" >&2
        exit 2
        ;;
    *)
        printf 'update.sh: install.sh exited %s; the service was left as it was\n' "$rc" >&2
        exit 1
        ;;
esac

# The stage has served its purpose: leaving it behind would let a later
# --from-stage re-apply a build that is already live.
if [ "$STAGE" -eq 1 ] && [ "$FROM_STAGE" -eq 1 ]; then
    _dir="$(stage_dir_for)"
    rm -rf "$_dir" 2>/dev/null || { [ -n "$SUDO" ] && "$SUDO" rm -rf "$_dir" 2>/dev/null; } || true
fi

# --- report -----------------------------------------------------------------

served="$("$installed_bin" --version 2>/dev/null || true)"
printf '\n'
info "${served:-karucode (no --version output)} is installed at ${installed_bin}"
printf 'Service: %s\n' "$("$SYSTEMCTL" is-active "$SERVICE" 2>/dev/null || true)"
printf 'Logs:    journalctl -u %s -f\n' "$SERVICE"
printf 'Roll back: sudo cp -p %s.old %s && sudo systemctl restart %s\n' \
    "$installed_bin" "$installed_bin" "$SERVICE"
if [ "$KARUCODE_DETACHED" = "1" ]; then
    printf 'This run log: %s/log\n' "${JOB_DIR:-$(job_dir_for)}"
fi
exit 0
