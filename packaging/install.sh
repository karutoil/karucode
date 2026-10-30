#!/bin/sh
# shellcheck disable=SC2317  # restore_unit and finish are only called from the EXIT trap
# karucode installer: binary + a systemd unit that actually starts, stays
# enabled across reboots, and survives upgrades.
#
#   curl -fsSLO https://raw.githubusercontent.com/karutoil/karucode/main/packaging/install.sh
#   sudo sh install.sh                          # latest release, /usr/local
#   sudo sh install.sh --workspace /srv/app     # the directory the agent may edit
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
#
# Exit codes:
#   0  installed, and (when systemd was requested) the service is running
#   1  usage, download, verification or install failure - nothing was changed,
#      or the systemd changes were rolled back
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

SERVICE_USER="karucode"
SERVICE_GROUP="karucode"
SERVICE_NAME="karucode"
WORKSPACE="/srv/karucode/workspace"
DATA_DIR="/var/lib/karucode"
HOST_ARG=""
PORT_ARG=""
ENV_FILE="/etc/karucode/karucode.env"

UNIT_DIR="/etc/systemd/system"
UNIT_PATH="${UNIT_DIR}/${SERVICE_NAME}.service"
DROPIN_DIR="${UNIT_PATH}.d"
DROPIN_PATH="${DROPIN_DIR}/10-install.conf"

USE_SYSTEMD=1
ENABLE=1
START=1
RESTART=1
ALLOW_JIT=0
DRY_RUN=0
FORCE_SYSTEMD=0
ARTIFACT=""
UNIT_SRC=""

TMPDIR_=""
BIN_DIR=""
UNIT_BAK=""
DROPIN_BAK=""
BIN_BAK=""
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
  --workspace <DIR>      directory the agent may read and edit
                         (default: /srv/karucode/workspace)
  --data-dir <DIR>       SQLite database and plugin data (default: /var/lib/karucode)
  --service-user <NAME>  run the service as this user (default: karucode, created
                         if missing). Use an existing account only if it already
                         owns --data-dir.
  --host <ADDR>          bind address (default: whatever the unit sets, 127.0.0.1)
  --port <PORT>          listen port (default: 8787)
  --env-file <PATH>      environment file (default: /etc/karucode/karucode.env)

systemd:
  --no-systemd           install only the binary: no user, no unit, no systemctl
  --no-enable            install the unit but do not enable it at boot
  --no-start             do not start it now
  --no-restart           leave a running instance on the old binary
  --allow-jit            set MemoryDenyWriteExecute=no so the agent can run
                         node/java/JIT runtimes (weaker hardening)
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
        --data-dir)        need_value "$@"; DATA_DIR="$2"; shift 2 ;;
        --data-dir=*)      DATA_DIR="${1#*=}"; shift ;;
        --service-user)    need_value "$@"; SERVICE_USER="$2"; SERVICE_GROUP="$2"; shift 2 ;;
        --service-user=*)  SERVICE_USER="${1#*=}"; SERVICE_GROUP="$SERVICE_USER"; shift ;;
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
        --force-systemd)   FORCE_SYSTEMD=1; shift ;;
        --dry-run)         DRY_RUN=1; shift ;;
        -h|--help)         usage; exit 0 ;;
        *)                 die "unknown argument: $1 (try --help)" ;;
    esac
done

[ -n "$PREFIX" ] || die "--prefix may not be empty"
[ -n "$WORKSPACE" ] || die "--workspace may not be empty"
[ -n "$DATA_DIR" ] || die "--data-dir may not be empty"
if [ "$SERVICE_USER" = "root" ]; then
    die "the service must not run as root: the agent executes arbitrary commands. Pick an unprivileged user."
fi
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
    if [ -d "$WORKSPACE" ] && ! can_write "$WORKSPACE"; then
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
    if ! id -u "$SERVICE_USER" >/dev/null 2>&1; then
        info "creating the ${SERVICE_USER} system group and user"
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
    if [ ! -d "$WORKSPACE" ]; then
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
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '    (dry-run) write the [Service] overrides (user, workspace, data dir, ExecStart) to %s\n' "$DROPIN_PATH"
    else
        install -d -m 0755 "$DROPIN_DIR"
        dropin_tmp="${TMPDIR_}/10-install.conf"
        {
            printf '# Generated by packaging/install.sh - rewritten on every run.\n'
            printf '# Put your own overrides in 20-local.conf; install.sh never touches it.\n'
            printf '[Service]\n'
            printf 'User=%s\n' "$SERVICE_USER"
            printf 'Group=%s\n' "$SERVICE_GROUP"
            printf 'WorkingDirectory=%s\n' "$WORKSPACE"
            # ReadWritePaths= is a list: reset it, then list exactly what we own.
            printf 'ReadWritePaths=\n'
            printf 'ReadWritePaths=%s\n' "$WORKSPACE"
            # StateDirectory=karucode already covers the default data dir; a
            # custom one has to be granted explicitly.
            if [ "$DATA_DIR" != "/var/lib/karucode" ]; then
                printf 'ReadWritePaths=%s\n' "$DATA_DIR"
                printf 'StateDirectory=\n'
            fi
            printf 'Environment=KARUCODE_DATA_DIR=%s\n' "$DATA_DIR"
            if [ -n "$HOST_ARG" ]; then
                printf 'Environment=KARUCODE_HOST=%s\n' "$HOST_ARG"
            fi
            if [ -n "$PORT_ARG" ]; then
                printf 'Environment=KARUCODE_PORT=%s\n' "$PORT_ARG"
            fi
            if [ "$ALLOW_JIT" -eq 1 ]; then
                printf '# --allow-jit: node/java/some Python extensions need W^X memory.\n'
                printf 'MemoryDenyWriteExecute=no\n'
            fi
            printf 'ExecStart=\n'
            printf 'ExecStart=%s/karucode\n' "$BIN_DIR"
        } > "$dropin_tmp"
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

if [ "$DRY_RUN" -eq 1 ]; then
    printf '\nDry run: nothing was downloaded, installed or written.\n\n'
    printf 'A real run would write %s/karucode and:\n' "$BIN_DIR"
    if [ "$USE_SYSTEMD" -eq 1 ]; then
        printf '  %s\n' "$UNIT_PATH"
        printf '  %s\n' "$DROPIN_PATH"
        printf '  %s (only if it does not exist yet)\n' "$ENV_FILE"
        printf '  %s and %s (only if they do not exist yet)\n' "$WORKSPACE" "$DATA_DIR"
        printf '  the %s system user and group (only if missing)\n' "$SERVICE_USER"
        printf '  and then: systemctl daemon-reload, enable, and start/restart %s\n' "$SERVICE_NAME"
    else
        printf '  (no systemd unit: --no-systemd or systemd was not detected)\n'
    fi
    exit 0
fi

if [ "$USE_SYSTEMD" -ne 1 ]; then
    cat <<EOF

Next steps (no systemd, nothing was enabled):

  ${BIN_DIR}/karucode --workspace /path/to/project

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
  * node/java crashing with SIGSEGV: re-run with --allow-jit

Files installed by this run:
  ${BIN_DIR}/karucode
  ${UNIT_PATH}
  ${DROPIN_PATH}
EOF
exit 2