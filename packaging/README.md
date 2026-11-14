# Deploying karucode

Operator notes for running karucode as a long-lived service: install, configure,
put it behind a proxy, back it up, upgrade it. For the build itself see the
top-level [README](../README.md); for the threat model see
[SECURITY.md](../SECURITY.md).

Files in this directory:

| File | Purpose |
| --- | --- |
| `install.sh` | Downloads a verified release tarball (or takes a local build), installs the binary, the unit and a site drop-in, enables the service and starts it |
| `update.sh` | Rebuilds a checkout of your own and moves the running service onto the new binary: fetch, build, database snapshot, then `install.sh` |
| `karucode.service` | systemd unit with the hardening that contains the agent |

`scripts/e2e_install.py` tests the installer end to end in a throwaway systemd
container: it installs, reboots the container, upgrades, and checks that the
service comes back on its own each time.

## Read this first: there is no filesystem sandbox

karucode runs the commands the model asks for with `/bin/sh -c <command>` **as the
service user**. The file tools are confined to the project, but shell commands are
not: a command can read `~/.ssh`, write anywhere the service user can, or reach the
network. The approval policy is a convenience, not a security boundary.

The shipped unit does **not** put a filesystem sandbox around that. `mv`, `cp` and
`rm` work across the filesystem, and no build fails with `EROFS` — which is the
point: a read-only root is a real wall in front of ordinary work, and keeping its
allowlist correct for every toolchain was a permanent maintenance burden.

So the deployment is only as safe as the account you give it:

* run it as **yourself** (the default — `install.sh` uses whoever ran it), never
  as root. This is the lever that replaces the filesystem sandbox: the agent
  cannot touch anything that account cannot. Running as a separate `karucode`
  account is *worse*, not better: it cuts the agent off from your toolchains,
  `~/.cargo`, `~/.local/bin` and your project directories, which is exactly the
  class of `Permission denied` failures this default avoids;
* `NoNewPrivileges=yes` stays on, so a setuid binary cannot escalate it to root;
* mount or expose only the directories you actually want it working in;
* if you need real isolation, run it in a container or VM with just the project
  mounted (see the `Dockerfile` at the repository root).

### Adding a filesystem sandbox yourself

If you *do* want one, add it in `20-local.conf`, which no tool rewrites:

```ini
# /etc/systemd/system/karucode.service.d/20-local.conf
[Service]
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=/srv/karucode/workspace /var/lib/karucode
```

```bash
sudo systemctl daemon-reload && sudo systemctl restart karucode
```

Two things to know before you do:

* that `ReadWritePaths=` list is now yours to maintain. Every new toolchain
  (`~/.cargo`, `~/.npm`, `~/.pnpm`, `~/.bun`, `~/.rustup`, plugin checkouts, a
  screenshot directory) needs an entry, and each omission surfaces as `EROFS`
  somewhere deep inside a build.
* use `ProtectSystem=no` / `ProtectHome=no` to switch either off later — an empty
  `ProtectSystem=` is *rejected* by systemd (`Failed to parse ProtectSystem=,
  ignoring: Invalid argument`) and silently leaves the strict value in force.

## Install

### With the script

```bash
curl -fsSLO https://raw.githubusercontent.com/karutoil/karucode/main/packaging/install.sh
sudo sh install.sh --workspace /srv/karucode/workspace
sudo sh install.sh --version 107 --port 8787
sh install.sh --no-systemd --prefix "$HOME/.local"      # binary only, no root
```

Before anything is written, `--dry-run` prints exactly what a real run would do:

```bash
sudo sh install.sh --workspace /srv/karucode/workspace --dry-run
```

The script downloads the tarball to a temporary file, verifies it against the
release's `SHA256SUMS`, and only then installs. A mismatch aborts with nothing
installed. It never pipes a download into a shell.

What a systemd install does, in order:

1. installs the binary to `<prefix>/bin/karucode`, keeping the previous one as
   `karucode.old`;
2. resolves the account to run as — the human who invoked it (see below) — and its
   real primary group. It creates the `karucode` system group and user *only* on
   the fallback path, when no human could be identified;
3. creates `--data-dir` if it is missing, owned by that account (and `--workspace`
   too, but only if you asked for one), warning instead of re-owning a directory
   that already exists;
4. writes a commented template to `/etc/karucode/karucode.env` **only if the file
   does not exist** — an existing one is never touched;
5. installs the unit to `/etc/systemd/system/karucode.service` (previous copy kept
   as `.bak`);
6. writes the site configuration — `User=`, `Group=`, the working directory, the
   data dir, the port and the `ExecStart=` path — to
   `/etc/systemd/system/karucode.service.d/10-install.conf`;
7. `daemon-reload`, `enable` (so it starts on boot), then `start` — or `restart`
   if the service was already running, so an upgrade does not leave the old
   binary serving;
8. waits for the unit to stay active and for `/api/health` to return HTTP `200`,
   or the expected `401` when authentication protects the route. No answer and
   every other status (including `403`) are failures.

If changing the unit, starting the service, or checking health fails, the previous
unit, drop-in, and binary are restored before exit. The binary replacement and
restoration use a same-directory temporary file followed by `mv`, never a copy
over a running executable. Exit codes: `0` installed and healthy (or deliberately
not started), `1` nothing installed (or rolled back), `2` start/health failed and rollback was
attempted. Startup failures also print the things worth checking.

**Everything except the binary has a default**, so `sudo sh install.sh` is the
whole install. There is no default workspace: the server starts with no project
and you add the first one in the web UI, which is also where the model endpoint
and key are set. Pick a directory for it wherever you keep projects.

Useful flags, all optional: `--workspace DIR` (pre-create and adopt one
directory as the first project), `--data-dir DIR` (default
`~/.local/share/karucode`, or the account's own home for a system account),
`--port N`, `--host ADDR`, `--service-user NAME` / `--service-group NAME` (default:
the account that ran the installer, and its real primary group; never root, never
created, never re-owned), `--no-start`, `--no-enable`, `--no-restart`, `--no-jit`,
`--binary PATH` (install a build you already have), `--tarball PATH`,
`--unit-file PATH`. `install.sh --help` lists them all.

`--dry-run` changes nothing and prints the `10-install.conf` it *would* write, so
the resolved `User=`, `Group=` and `KARUCODE_DATA_DIR=` can be checked before
anything is installed. It works without root.

`MemoryDenyWriteExecute=no` is written by default because V8, the JVM and some
Python C extensions allocate executable memory: with the knob on, every `node`,
`java` or `vite build` in an agent session dies with a V8 fatal error. Pass
`--no-jit` if you want the stricter setting and never run those.

### Why the unit is not edited in place

The paths that must match your machine (`WorkingDirectory=`, the data dir and
`ExecStart=`) live in a drop-in, not in the unit:

```text
/etc/systemd/system/karucode.service        # shipped, replaced on every install
/etc/systemd/system/karucode.service.d/
    10-install.conf                        # generated, rewritten on every install
    20-local.conf                          # yours, never touched
```

Editing the unit directly is the classic way this breaks: the installer replaces
the file on the next upgrade, and the service then fails to start with
`status=200/CHDIR` because the workspace path it needs is gone. Put local
overrides in `20-local.conf`, for example:

```ini
# /etc/systemd/system/karucode.service.d/20-local.conf
[Service]
# The toolchain PATH the agent's shell needs (see below).
Environment=PATH=/srv/karucode/.cargo/bin:/usr/local/bin:/usr/bin:/bin
# The agent builds the frontend, so it needs to run node.
MemoryDenyWriteExecute=no
```

`sudo systemctl daemon-reload && sudo systemctl restart karucode` after editing it.

### Running as an existing account (the default)

That is what happens out of the box: the unit runs as the account that invoked the
installer, with the data directory in that account's `~/.local/share/karucode`. If
you already ran karucode by hand, this keeps your existing database — the provider
API key and every setting live in the database, not in the env file, so pointing at
a *different* data directory would be a different (empty) instance.

```bash
# what a plain install already does
sudo sh install.sh
```

The installer resolves "the account that invoked it" from, in order, `$SUDO_USER`,
`$LOGNAME`, and the session login uid (`/proc/self/loginuid`); `$USER` counts only
when it is not `root`, because under `sudo` it always is. Every candidate has to be
an existing, non-root account. **Root is never used, in any spelling**: passing
`--service-user root`, `0`, `00`, `root:root` or `0:0` exits `1`.

The final summary names the account it settled on, so you can check it instead of
trusting it:

```text
Service account: you:you (who ran this installer)
Data directory:  /home/you/.local/share/karucode
```

`systemctl show -p User -p Group karucode` must agree. The group is the account's
real primary gid from `/etc/passwd` — not a group assumed to share the account's
name — and `--service-group` overrides it.

`--service-user` names an account that already exists; the installer does not
create accounts, and it exits `1` for a name it cannot resolve rather than letting
systemd fail later with `status=217/USER`. Nothing about that account is re-owned.

**The one silent failure mode, made loud:** a bare root shell — `sudo -i`, or a
container/image build — has no `$SUDO_USER` and no login uid, so there is no human
to name. The installer then falls back to the dedicated `karucode` system account
(creating it and `/var/lib/karucode` if missing) and prints a six-line warning
saying so, plus `(fallback: no human could be identified)` in the summary. If you
see that and did not mean it, re-run as yourself, or pass `--service-user <name>`.

A dedicated system account is still selectable for a shared or locked-down host, but
then you own the consequences: give it read access to the directories it must work
in, or it will fail with `Permission denied` exactly where a real user's home is
private (`~/.local/bin` is `700` on many systems). Its data directory defaults to
that account's own home, which for a system account created by `useradd --system
--home-dir /var/lib/karucode` is `/var/lib/karucode` — not
`/var/lib/karucode/.local/share/karucode` — because a service account's home *is*
its data directory.

```bash
sudo sh install.sh --service-user karucode     # -> /var/lib/karucode
```

One thing the unit needs in `20-local.conf`:

**`Environment=PATH=…`.** systemd hands the service the bare default
`PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin`, not your interactive one.
`cargo` (in `~/.cargo/bin`), `pnpm`, `bun` and the nvm `node` are then *not found*
by the agent's shell, even though they work in your terminal. The toolchain caches
themselves need no grants: the unit does not sandbox the filesystem.

```ini
[Service]
Environment=PATH=/home/you/.local/bin:/home/you/.cargo/bin:/home/you/.nvm/versions/node/v22.22.2/bin:/usr/local/bin:/usr/bin:/bin
```

### Manually

```bash
VERSION=107
TARGET=x86_64-unknown-linux-gnu
NAME="karucode-${VERSION}-${TARGET}"
BASE="https://github.com/karutoil/karucode/releases/download/v${VERSION}"

curl -fsSLO "${BASE}/${NAME}.tar.gz"
curl -fsSLO "${BASE}/SHA256SUMS"
sha256sum -c --ignore-missing SHA256SUMS     # must print OK
tar -xzf "${NAME}.tar.gz"

sudo install -m 0755 "${NAME}/karucode" /usr/local/bin/karucode
sudo groupadd --system karucode
sudo useradd --system --gid karucode --home-dir /var/lib/karucode \
     --shell /usr/sbin/nologin karucode
sudo install -d -m 0750 -o karucode -g karucode /srv/karucode/workspace
sudo install -m 0644 packaging/karucode.service /etc/systemd/system/
sudo install -d -m 0755 /etc/systemd/system/karucode.service.d

# The site paths go in the drop-in, not in the unit.
sudo tee /etc/systemd/system/karucode.service.d/10-install.conf >/dev/null <<'EOF'
[Service]
WorkingDirectory=/srv/karucode/workspace
EOF

sudo systemctl daemon-reload
sudo systemctl enable --now karucode
```

Only `x86_64-unknown-linux-gnu` is published by the release workflow today. On
aarch64, build from source (`cargo build --release -p kc-server`) and either pass
`--binary target/release/karucode` to `install.sh` or copy it into place yourself.

### From source

```bash
cargo build --release -p kc-server        # embeds web/dist; needs Node 22.22+ once
sudo install -m 0755 target/release/karucode /usr/local/bin/karucode
# then the same unit + drop-in steps as above
```

Or let the installer do the systemd part with the build you just produced:

```bash
sudo sh packaging/install.sh --binary target/release/karucode \
     --workspace /srv/karucode/workspace
```

## Configure

The unit reads `/etc/karucode/karucode.env` (mode `0600`, owned by root). It is
optional — the service starts without it — but you almost certainly want it:

```bash
sudo install -d -m 0755 /etc/karucode
sudo install -m 0600 /dev/null /etc/karucode/karucode.env
sudoedit /etc/karucode/karucode.env
```

```sh
# Required for anything beyond a local single-user trial.
KARUCODE_AUTH_TOKEN=<32+ random chars>      # openssl rand -hex 32
# Optional: a separate token for admin-only endpoints.
KARUCODE_ADMIN_TOKEN=<32+ random chars>

# The model connection (any OpenAI-compatible gateway).
KARUCODE_BASE_URL=https://api.example.com/v1
KARUCODE_API_KEY=sk-...
KARUCODE_MODEL=gpt-4o-mini

# Optional allowlist for where a project may be *created*. Unset means any
# directory except `/`. Recommended here: this deployment is token-protected and
# reachable, so pin the list to the directories you actually run projects in.
KARUCODE_PROJECT_ROOTS=/srv/karucode/workspace
# The default workspace for sessions that are not attached to a project.
KARUCODE_WORKSPACE=/srv/karucode/workspace

# SQLite database and plugin data. Must match StateDirectory= in the unit.
KARUCODE_DATA_DIR=/var/lib/karucode

# Pin settings from the environment so the UI cannot change them.
KARUCODE_LOCK_SETTINGS=1
```

Notes:

* `KARUCODE_AUTH_TOKEN` must be at least 16 characters. The server **refuses to
  start** on a non-loopback bind without it, so it is mandatory if you change
  `KARUCODE_HOST` away from `127.0.0.1`.
* `KARUCODE_DATA_DIR` should be `/var/lib/karucode` under this unit: the default is
  `$HOME/.local/share/karucode`, and the service user's `$HOME` is
  `/var/lib/karucode` with no login shell. Any directory works as long as the
  service user owns it.
* The data directory holds the SQLite database **and the provider API key** (it is
  stored in the settings table). Treat it as a secret; the server forces mode
  `0700`.
* A config file is optional. If you want one, put it at
  `/etc/karucode/config.toml` and set `KARUCODE_CONFIG=/etc/karucode/config.toml`.
  Do not point `KARUCODE_CONFIG` at a file that does not exist — the server exits
  at startup if the path it is given is missing. See
  [config.example.toml](../config.example.toml) for every key.
* Precedence, highest first: UI overrides saved in the database → environment →
  config file → defaults. `KARUCODE_LOCK_SETTINGS=1` removes the first layer.

Then point the service at your workspace. `install.sh` does this for you; by hand,
write it to the drop-in (not the unit, which is replaced on upgrade):

```ini
# /etc/systemd/system/karucode.service.d/10-install.conf
[Service]
WorkingDirectory=/srv/karucode/workspace
```

It must exist and be owned by `karucode`. That is where sessions start; the agent
is not confined to it, so run the service as an account that cannot reach anything
you would mind it touching.

```bash
sudo install -d -m 0750 -o karucode -g karucode /srv/karucode/workspace
sudo systemctl daemon-reload
sudo systemctl enable --now karucode
journalctl -u karucode -f
```

A unit whose `WorkingDirectory=` does not exist fails at startup with
`status=200/CHDIR` before the binary runs at all, which is why the installer
creates it rather than documenting it.

Check it:

```bash
curl -s localhost:8787/api/health
# with a token configured:
curl -s -H "Authorization: Bearer $KARUCODE_AUTH_TOKEN" localhost:8787/api/health
```

`/api/health` answers before any model is configured, which makes it the right
probe for a supervisor. It sits behind the auth middleware, so a probe against a
token-protected instance must send the bearer token or it gets a `401`.

## Reverse proxy (there is no TLS in the binary)

karucode speaks plain HTTP. Keep `KARUCODE_HOST=127.0.0.1` and terminate TLS in a
reverse proxy on the same host.

nginx:

```nginx
server {
    listen 443 ssl;
    server_name karucode.example.com;

    ssl_certificate     /etc/letsencrypt/live/karucode.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/karucode.example.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:8787;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }

    # Server-sent events: buffering would stall the stream.
    location /api/events {
        proxy_pass http://127.0.0.1:8787;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_buffering off;
        proxy_read_timeout 3600s;
    }
}
```

* Set `KARUCODE_AUTH_TOKEN` before exposing the service. With a token configured
  the `Host` allowlist is skipped and the token is the gate.
* `KARUCODE_ALLOWED_HOSTS` is only needed for the *unauthenticated* loopback case:
  when the server is bound to loopback and no token is set, it only answers for
  `Host: 127.0.0.1`, `localhost` or `[::1]`, and a proxy that forwards the public
  hostname gets a `403`. Either set a token (recommended) or list the proxy's
  hostname: `KARUCODE_ALLOWED_HOSTS=karucode.example.com`.
* Do not expose `/metrics` publicly; it is behind the same auth layer, so a token
  protects it too.

## Backup

There is no `karucode backup` subcommand yet, so use SQLite's own online backup.
`VACUUM INTO` writes a consistent snapshot while the server is running:

```bash
sudo install -d -m 0700 /var/backups/karucode
sudo -u karucode sqlite3 /var/lib/karucode/karucode.db \
  "VACUUM INTO '/var/backups/karucode/karucode-$(date +%F-%H%M).db'"
```

The snapshot is a single file with no `-wal`/`-shm` sidecars, so it is safe to
copy off the host. It contains sessions, settings **and the provider API key** —
encrypt it at rest.

Restore:

```bash
sudo systemctl stop karucode
sudo -u karucode cp /var/backups/karucode/karucode-2025-01-01-1200.db \
     /var/lib/karucode/karucode.db
sudo rm -f /var/lib/karucode/karucode.db-wal /var/lib/karucode/karucode.db-shm
sudo systemctl start karucode
```

A cold copy also works if you stop the service first (`systemctl stop karucode`,
then copy the whole `/var/lib/karucode` directory). Do not copy `karucode.db`
while the server is running without `VACUUM INTO`: in WAL mode the `.db` file
alone is not a complete snapshot — the recent writes live in `karucode.db-wal`,
so a bare copy of the `.db` can restore an empty database.

These commands need `sqlite3` on the host (`apt install sqlite3`); karucode
itself does not ship it.

Only one process may use a data directory at a time — the server holds an
exclusive lock on `/var/lib/karucode/karucode.lock` and refuses to start a second
instance on the same directory. That is also why you cannot run the container and
the systemd unit against the same volume.

## Upgrade

karucode versions itself by **commit count**: every commit on `main` is a release,
build 107 is the 107th commit, and the tag is `v107`. `karucode --version` prints
the build, the commit and its date, and it is the number to compare with a release.

```bash
# 1. Back up first: there is no automatic pre-migration snapshot.
sudo -u karucode sqlite3 /var/lib/karucode/karucode.db \
  "VACUUM INTO '/var/backups/karucode/pre-upgrade-$(date +%F-%H%M).db'"

# 2. Either let the running install update itself from the release manifest:
karucode update --check          # 0 up to date, 3 a newer build exists
sudo karucode update --apply --data-dir /var/lib/karucode --restart

# ... or install a release explicitly. Same command as the first install: it
#    verifies the checksum, replaces the binary, rewrites the unit and the
#    drop-in, and restarts the service onto the new binary.
sudo sh install.sh --version 107 --workspace /srv/karucode/workspace

# 3. Watch it come up.
journalctl -u karucode -n 50
```

`karucode update` is the lighter of the two: it does **not** rewrite the unit or
the drop-in, it replaces the binary and (with `--restart`) restarts the unit. The
server checks for a newer build on startup and every six hours by default and
says so in **Settings → Updates**; when the service account cannot write
`/usr/local/bin/karucode` - the normal case, because the installer runs as root -
the server stages and verifies the download and that panel prints exactly the
`sudo` command to run. The previous binary is kept as `karucode.old`.

```bash
# Off, or pointed at a mirror you control:
KARUCODE_UPDATE_CHECK=0
KARUCODE_UPDATE_MANIFEST_URL=https://mirror.example.com/karucode/updates.json
```

A container is updated by rebuilding the image, not by any of this.

### Updating a service that runs your own build

`install.sh` installs a binary; it does not build one. If the unit runs something
you compiled from a checkout — the normal case while you are following `main`
rather than a release — use `update.sh`. It is one command, and it does the four
things that are easy to get wrong by hand:

```bash
sudo sh packaging/update.sh            # pull, build, stage, install, restart
sh packaging/update.sh --check         # exit 3 if commits are waiting; changes nothing
sudo sh packaging/update.sh --dry-run  # print the whole plan; build nothing
```

1. **It builds as you, not as root.** `sudo ./update.sh` that runs `cargo` as
   root leaves root-owned files in `target/` and in `~/.cargo`, and your next
   build as yourself fails on permissions. Only the stage, the snapshot and
   `install.sh` are elevated; the fetch and the build run as the owner of the
   checkout.
2. **It reads the running unit instead of guessing.** `ExecStart=`, `User=`,
   `WorkingDirectory=`, the `KARUCODE_*` lines and `MemoryDenyWriteExecute=` are
   taken from `systemctl show` and passed to `install.sh`. The defaults compiled
   into `install.sh` (`/srv/karucode/workspace`, user `karucode`) are for a fresh
   host; without this, the first update would rewrite `10-install.conf` to point
   at a workspace that does not exist and the service would die with
   `status=200/CHDIR`. A workspace that is not on disk is a hard error, not a
   guess.
3. **It snapshots the database first.** `VACUUM INTO <data-dir>/backups/pre-update-<stamp>.db`
   runs before anything restarts, because there is no automatic pre-migration
   snapshot. `sqlite3` is mandatory when a database exists; the update stops if it
   is missing unless you explicitly pass `--no-backup`. The backup directory is
   enforced and verified as `0700`, and the snapshot as `0600`. `VACUUM INTO`
   works while the server is running and the result is one file with no
   `-wal`/`-shm` sidecars.
4. **It refuses to ship what you did not commit.** Modified tracked files are
   rejected (`--allow-dirty` overrides those only). Non-ignored untracked build
   inputs are always rejected; ignored build output such as `target/` and
   `web/dist/` is allowed. What lands in `/usr/local/bin` is therefore the
   revision you think it is.
5. **It stages before it switches, and survives losing the terminal.** See below.

Every systemd step is still `install.sh`'s: keeps `karucode.old`, rewrites the
unit and the drop-in, clears a stale `StartLimitBurst`, restarts a running
service onto the new binary, and only reports success for an intentional health
status: HTTP `200`, or `401` when auth protects the route. Any other response, or
no response, triggers rollback. There is deliberately no second copy of that
logic to drift.

#### Staged, then switched

Everything that can fail happens while the old binary is still serving. The build
and the unit file are copied into a stage directory and **those copies** are
validated: the binary has to run (`--version`), the unit has to contain a
`[Unit]` section, and a sha256 of the staged binary is written to a manifest.
Only then does the switch move the binary, rewrite `10-install.conf` and restart
the unit.

```bash
sudo sh packaging/update.sh --stage-only   # build and validate; stop there
sudo sh packaging/update.sh --from-stage   # switch onto it; no rebuild, no fetch
```

* `--stage-only` is the maintenance-window form. It prints the staged version and
  sha256 and leaves the running service completely alone; a bad build cannot take
  it down.
* `--from-stage` applies exactly those bytes. It refuses if the staged binary no
  longer matches the recorded checksum, refuses an incomplete stage (no
  manifest), and cannot be combined with `--build-only` or `--check`. After a
  successful switch the stage is removed, so the same stage cannot be applied
  twice.
* The stage is `<data-dir>/stage` (`--stage-dir` to move it), root-owned and
  world-readable: the build runs as you and `install.sh` reads it as root, so the
  bytes have to cross that boundary as files rather than as a path under your
  home. `--no-stage` skips staging and installs straight from the build.

A stale stage is cleared at the start of the next `--stage-only`, and the
manifest is written last, so a failed re-stage can never leave a manifest
describing bytes that were never validated.

#### Surviving the terminal

A run that builds or switches **detaches itself into its own session** before
doing anything, with its own log, so closing the terminal or dropping an SSH
connection cannot stop an update half way through the switch.

```text
<data-dir>/update-job/
    log       everything the run printed
    pid       the detached process
    status    the exit code, written when it finishes
```

The shell you started it from waits and forwards that exit code, so
`sudo sh packaging/update.sh` still exits `0`, `1` or `2` as documented. From
another terminal:

```bash
sh packaging/update.sh --follow    # tail the log; Ctrl-C stops the tail, not the job
```

Detaching is deliberately skipped when it would do more harm than good: with no
terminal (a `sudo` password prompt could not be answered, so the run would hang
invisibly), when `sudo -n` cannot escalate without a password, and for
`--check`, `--dry-run`, `--build-only` and `--stage-only`, where there is nothing
live to protect. Force either behaviour with `--detach` / `--no-detach`.

Two updates cannot run at once: a lock directory beside the data directory
(`<data-dir>/../.<name>.update.lock`) refuses the second run and names the pid
holding it. A lock whose process is gone is reported as stale with the exact
command to remove it. If that directory is not writable at all, the script says
so once and continues rather than refusing to run.

| Flag | Effect |
| --- | --- |
| `--check` | Is there anything to update? Changes nothing; exit `3` if there is |
| `--dry-run` | Print the fetch, build and install plan; change nothing |
| `--stage-only` | Build and validate into the stage, then stop before the switch |
| `--from-stage` | Switch onto the staged bytes; no rebuild, no fetch |
| `--no-stage` | Install straight from the build, skipping the stage |
| `--stage-dir <DIR>` | Put the stage somewhere other than `<data-dir>/stage` |
| `--detach` / `--no-detach` | Force or forbid running in the background |
| `--follow` | Attach to a detached run's log |
| `--no-pull` | Build the working tree as it is (offline, or a local branch) |
| `--ref <REF>` | Build a tag/commit instead of the current branch (detached HEAD) |
| `--build-only` | Compile and stop; the service is not touched |
| `--skip-build` | Install the binary already in `target/release` |
| `--allow-dirty` | Permit modified tracked files; non-ignored untracked inputs are still refused |
| `--no-backup` | Explicitly skip the snapshot (the only way to proceed without `sqlite3` when a database exists) |
| `--backup-dir <DIR>` | Write the snapshot somewhere other than `<data-dir>/backups` |
| `--force` | Reinstall even when the binary and the unit are unchanged |
| `--workspace`, `--data-dir`, `--service-user`, `--host`, `--port`, `--prefix` | Override what the unit says |

Exit codes: `0` updated and healthy, `1` nothing changed (untracked/dirty tree,
build or backup failure, no unit, refused install), `2` start/health failed and
`install.sh` restored the previous unit and binary, `3` `--check` found commits
you do not have. A detached run with a missing or malformed status file exits `1`:
absence of a status is never interpreted as success.

Two things it will not do for you:

* **It does not run from inside the service.** The unit sets
  `NoNewPrivileges=yes`, so `sudo` cannot escalate from an agent session; the
  script detects that and says so. Use a normal login shell on the host.
* **It does not turn a failed upgrade into success.** If the *build* compiles but
  the service will not start or fails health, `install.sh` restores the previous
  binary and unit before `update.sh` exits `2`. `karucode.old` and
  `pre-update-*.db` remain available for inspection or manual recovery.

What survives an upgrade, and what does not:

* **Survives:** `/etc/karucode/karucode.env`, `/var/lib/karucode` (database and
  provider key), `20-local.conf`, and the workspace. The installer creates each of
  them only when it is missing.
* **Rewritten:** the unit file (previous copy kept as `karucode.service.bak`) and
  `10-install.conf`. That is deliberate: they are the installer's files, so the
  paths are never lost, which is the failure mode the old "edit the unit by hand"
  flow had.
* **Kept for rollback:** `/usr/local/bin/karucode.old`.

If the service does not come back healthy, the installer exits `2` and attempts
to restore the previous binary and unit. Startup failures print the checklist.
In-flight runs are cancelled on
`SIGTERM`; on the next start, leftover `running` runs are marked `interrupted`
and pending approvals `expired` — nothing is silently retried.

Roll back a bad release:

```bash
sudo systemctl stop karucode
sudo install -m 0755 /usr/local/bin/karucode.old /usr/local/bin/karucode.rollback
sudo mv -f /usr/local/bin/karucode.rollback /usr/local/bin/karucode
sudo systemctl start karucode
```

The temporary file and atomic rename avoid truncating or writing over an
executable that is still mapped by a running process.

## Uninstall / start over

`install.sh` never deletes anything, so a full removal is these six commands.
Check each path before running it — if you moved the data directory with
`--data-dir`, use that path instead, and **do not** remove it unless you mean to
drop the sessions and the stored provider key.

```bash
sudo systemctl disable --now karucode
sudo rm -f /etc/systemd/system/karucode.service /etc/systemd/system/karucode.service.bak
sudo rm -rf /etc/systemd/system/karucode.service.d
sudo rm -f /usr/local/bin/karucode /usr/local/bin/karucode.old
sudo systemctl daemon-reload && sudo systemctl reset-failed

# Only if you want the data gone too (database + provider key + plugins):
sudo rm -rf /var/lib/karucode /etc/karucode
# And the service account:
sudo userdel karucode && sudo groupdel karucode
```

That leaves no trace in systemd (`systemctl list-unit-files | grep karucode`
prints nothing) and does not touch a checkout, a container, or any other
karucode instance you run.

## Container

The repository root has a multi-stage `Dockerfile` (Node builds the UI, Rust
builds the binary, Debian slim runs it as uid 10001). It is a packaging boundary,
not a sandbox — see the comment block at the top of the Dockerfile.

```bash
docker build -t karucode:latest .
docker run -d --name karucode -p 127.0.0.1:8787:8787 \
  --cap-drop=ALL --security-opt=no-new-privileges \
  -v karucode-data:/data -v /srv/projects:/workspace \
  -e KARUCODE_AUTH_TOKEN="$(openssl rand -hex 32)" \
  -e KARUCODE_BASE_URL=https://api.example.com/v1 \
  -e KARUCODE_API_KEY=sk-... -e KARUCODE_MODEL=gpt-4o-mini \
  karucode:latest
```

The image sets `KARUCODE_HOST=0.0.0.0`, which makes `KARUCODE_AUTH_TOKEN`
mandatory — the container exits at startup without it. The `/data` volume holds
the database and the provider key. Never mount the Docker socket into it.

The image deliberately does **not** include `sqlite3` (it is not needed at
runtime). To back up a container deployment, run the `VACUUM INTO` command on the
host against the volume path, or from a throwaway sqlite3 container:

```bash
docker run --rm -v karucode-data:/data -v /var/backups/karucode:/backup \
  keinos/sqlite3 sqlite3 /data/karucode.db "VACUUM INTO '/backup/karucode.db'"
```

## Troubleshooting

| Symptom | Cause |
| --- | --- |
| `refusing to bind ... without KARUCODE_AUTH_TOKEN` | Non-loopback bind with no token; set one (≥16 chars) |
| `workspace ... is not usable` | `WorkingDirectory=` / `KARUCODE_WORKSPACE` does not exist |
| `could not open the database at ...` | Permissions on `/var/lib/karucode`, or a foreign file |
| `config file ... does not exist` | `KARUCODE_CONFIG` points at a missing file; create it or unset it |
| `port ... in use` | Another instance is running |
| `403 permission_denied` behind a proxy | Unauthenticated loopback bind; set a token or `KARUCODE_ALLOWED_HOSTS` |
| `401` from `/api/health` | The probe is not sending the bearer token |
| Agent commands fail with `EPERM` | `SystemCallFilter=@system-service`; check `journalctl -u karucode` |
| `node`/`java` crash with `SIGSEGV` | `MemoryDenyWriteExecute=yes`; set it to `no` if the agent must run JIT runtimes (`install.sh --allow-jit`) |
| `status=200/CHDIR` and the unit dies before logging anything | `WorkingDirectory=` does not exist; create it and own it by the service user |
| `status=217/USER`, or `ProtectHostname=yes is configured, but UTS namespace setup is prohibited` | the unit is running inside an **unprivileged container**: namespace sandboxing needs `CAP_SYS_ADMIN`. Use the `Dockerfile` there, or `podman run --privileged` for a throwaway test container |
| `start of the service was attempted too often` | the unit failed earlier and hit `StartLimitBurst=5`; `sudo systemctl reset-failed karucode` (the installer now does this before it starts or restarts) |
| the installer says `active` but nothing answers on the port | `Type=simple` reports `active` as soon as the process forks. The installer waits for `/api/health`, so a `no answer` line there means the server bound somewhere else — check `ss -ltnp`, `KARUCODE_HOST`/`KARUCODE_PORT`, and whether another process owns the port |
| it works, but it is not running after a reboot | `systemctl is-enabled karucode` must print `enabled`; if it prints `disabled`, the install ran with `--no-enable` |
| a change to `20-local.conf` had no effect | `sudo systemctl daemon-reload` is required, and drop-in filenames must end in `.conf` |
| the agent cannot run `cargo`, `node`, `pnpm` or `bun` inside a session, though they work in your terminal | systemd gives the service the *default* PATH, not yours: set `Environment=PATH=…` in `20-local.conf` |
| `mv`/`cp`/`rm` fail with `Read-only file system` (`EROFS`) | you added a filesystem sandbox in `20-local.conf`: widen `ReadWritePaths=` there, or set `ProtectSystem=no` |
