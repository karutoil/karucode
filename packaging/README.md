# Deploying karucode

Operator notes for running karucode as a long-lived service: install, configure,
put it behind a proxy, back it up, upgrade it. For the build itself see the
top-level [README](../README.md); for the threat model see
[docs/SECURITY.md](../docs/SECURITY.md).

Files in this directory:

| File | Purpose |
| --- | --- |
| `install.sh` | Downloads a verified release tarball (or takes a local build), installs the binary, the unit and a site drop-in, enables the service and starts it |
| `karucode.service` | systemd unit with the hardening that contains the agent |

`scripts/e2e_install.py` tests the installer end to end in a throwaway systemd
container: it installs, reboots the container, upgrades, and checks that the
service comes back on its own each time.

## Read this first: the agent's shell is not sandboxed

karucode runs the commands the model asks for with `/bin/sh -c <command>` **as the
service user**. Workspace path confinement applies to the file tools, not to shell
commands: a command can read `~/.ssh`, write outside the workspace, or reach the
network. The approval policy is a convenience, not a security boundary.

So the deployment is only as safe as the account and the filesystem you give it:

* run it as a dedicated unprivileged user (`karucode`), never as root;
* `ProtectSystem=strict` + `ReadWritePaths=` in the unit is your main lever — the
  agent can only write inside the paths you list there;
* `NoNewPrivileges=yes` stops it from escalating to root through a setuid binary;
* mount or expose only the project directory you want it to touch;
* if you need real isolation, run it in a container or VM with just the project
  mounted (see the `Dockerfile` at the repository root).

## Install

### With the script

```bash
curl -fsSLO https://raw.githubusercontent.com/karutoil/karucode/main/packaging/install.sh
sudo sh install.sh --workspace /srv/karucode/workspace
sudo sh install.sh --version 0.1.0 --port 8787
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
2. creates the `karucode` system group and user if they are missing (an explicit
   group, because `Group=` in the unit is explicit);
3. creates `--workspace` and `--data-dir` if they are missing, owned by that
   user, and warns instead of re-owning a directory that already exists;
4. writes a commented template to `/etc/karucode/karucode.env` **only if the file
   does not exist** — an existing one is never touched;
5. installs the unit to `/etc/systemd/system/karucode.service` (previous copy kept
   as `.bak`);
6. writes the site configuration — the workspace, the data dir, the port and the
   `ExecStart=` path — to
   `/etc/systemd/system/karucode.service.d/10-install.conf`;
7. `daemon-reload`, `enable` (so it starts on boot), then `start` — or `restart`
   if the service was already running, so an upgrade does not leave the old
   binary serving;
8. waits for the unit to stay active and for `/api/health` to answer, and only
   then reports success.

If step 5–7 fails, the previous unit and drop-in are restored. Exit codes:
`0` installed and running, `1` nothing installed (or rolled back), `2` installed
but the service did not come up — the script then prints the four things worth
checking.

Useful flags: `--workspace DIR`, `--data-dir DIR`, `--port N`, `--host ADDR`,
`--service-user NAME`, `--no-start`, `--no-enable`, `--no-restart`,
`--allow-jit`, `--binary PATH` (install a build you already have),
`--tarball PATH`, `--unit-file PATH`. `install.sh --help` lists them all.

### Why the unit is not edited in place

The two paths that must match your machine (`WorkingDirectory=`,
`ReadWritePaths=`) live in a drop-in, not in the unit:

```
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
# A build cache the agent needs to write to, on top of the workspace.
ReadWritePaths=/srv/karucode/.cargo
# The agent builds the frontend, so it needs to run node.
MemoryDenyWriteExecute=no
```

`sudo systemctl daemon-reload && sudo systemctl restart karucode` after editing it.

### Running as an existing account

If you already ran karucode by hand, point the unit at the same account and data
directory instead of creating a fresh one — the provider API key and every
setting live in the database, not in the env file, so a new data directory is a
different (empty) instance:

```bash
sudo sh install.sh --service-user "$USER" \
     --data-dir "$HOME/.local/share/karucode" \
     --workspace "$HOME/karucode" --port 8787 --allow-jit
```

Two things the unit then needs, both in `20-local.conf`:

1. **`Environment=PATH=…`.** systemd hands the service the bare default
   `PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin`, not your interactive
   one. `cargo` (in `~/.cargo/bin`), `pnpm`, `bun` and the nvm `node` are then
   *not found* by the agent's shell, even though they work in your terminal.
2. **`ReadWritePaths=` for the toolchain caches.** `ProtectHome=read-only` makes
   `~/.cargo`, `~/.npm` and `~/.cache` read-only, so `cargo add` / `npm install`
   fail with `EPERM`. Grant those three (caches only — never the whole home).

```ini
[Service]
Environment=PATH=/home/you/.local/bin:/home/you/.cargo/bin:/home/you/.nvm/versions/node/v22.22.2/bin:/usr/local/bin:/usr/bin:/bin
ReadWritePaths=/home/you/.cargo
ReadWritePaths=/home/you/.npm
ReadWritePaths=/home/you/.cache
```

### Manually

```bash
VERSION=0.1.0
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
ReadWritePaths=
ReadWritePaths=/srv/karucode/workspace
EOF

sudo systemctl daemon-reload
sudo systemctl enable --now karucode
```

Only `x86_64-unknown-linux-gnu` is published by the release workflow today. On
aarch64, build from source (`cargo build --release -p kc-server`) and either pass
`--binary target/release/karucode` to `install.sh` or copy it into place yourself.

### From source

```bash
cargo build --release -p kc-server        # embeds web/dist; needs Node 20+ once
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
* `KARUCODE_DATA_DIR` must be `/var/lib/karucode` under this unit: the default is
  `$HOME/.local/share/karucode`, and `$HOME` is not writable with
  `ProtectSystem=strict`.
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
ReadWritePaths=
ReadWritePaths=/srv/karucode/workspace
```

Both paths must name the same directory, it must exist, and it must be owned by
`karucode`. `ReadWritePaths=` is the only place outside the state directory the
agent can write to — keep it narrow.

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

```bash
# 1. Back up first: there is no automatic pre-migration snapshot.
sudo -u karucode sqlite3 /var/lib/karucode/karucode.db \
  "VACUUM INTO '/var/backups/karucode/pre-upgrade-$(date +%F-%H%M).db'"

# 2. Install the new release. Same command as the first install: it verifies the
#    checksum, replaces the binary, rewrites the unit and the drop-in, and
#    restarts the service onto the new binary.
sudo sh install.sh --version 0.2.0 --workspace /srv/karucode/workspace

# 3. Watch it come up.
journalctl -u karucode -n 50
```

What survives an upgrade, and what does not:

* **Survives:** `/etc/karucode/karucode.env`, `/var/lib/karucode` (database and
  provider key), `20-local.conf`, and the workspace. The installer creates each of
  them only when it is missing.
* **Rewritten:** the unit file (previous copy kept as `karucode.service.bak`) and
  `10-install.conf`. That is deliberate: they are the installer's files, so the
  paths are never lost, which is the failure mode the old "edit the unit by hand"
  flow had.
* **Kept for rollback:** `/usr/local/bin/karucode.old`.

If the service does not come back, the installer exits `2` and prints the
checklist; nothing is silently left broken. In-flight runs are cancelled on
`SIGTERM`; on the next start, leftover `running` runs are marked `interrupted`
and pending approvals `expired` — nothing is silently retried.

Roll back a bad release:

```bash
sudo cp -p /usr/local/bin/karucode.old /usr/local/bin/karucode
sudo systemctl restart karucode
```

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
| `cargo add` / `npm install` fail with `EPERM` | `ProtectHome=read-only`; add `ReadWritePaths=` for `~/.cargo`, `~/.npm`, `~/.cache` |
