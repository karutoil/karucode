# Security model and trust boundaries

karucode is a **single-user, local-first coding agent**. It runs commands on the
machine it is installed on, as the user that started it. This document states
exactly what is and is not protected, so you can decide where to run it.

## Default posture

* Binds to `127.0.0.1` (loopback) by default. It refuses to start on a non-loopback
  address unless `KARUCODE_AUTH_TOKEN` is set (≥ 16 characters).
* One process serves the API, the SSE stream and the built UI. No other runtime
  service is required.
* The SQLite database and data directory are created with restrictive
  permissions (`0700` directory, `0600` file) because the settings table may hold
  an API key.

## What is protected

**Network exposure.** Loopback-only by default. With a token configured, API,
metrics, and terminal routes require `Authorization: Bearer <token>`. The public
SPA shell and static assets are deliberately readable so the browser can render
its authentication screen; that exposes no transcript or project data. The SSE
and terminal endpoints additionally accept `?token=` because browser stream and
WebSocket clients cannot reliably attach a header. Artifact frames instead use
their random per-artifact `?capability=`, never the master token. Without a
token, anyone who can reach the port can use the agent.

**Browser-origin attacks (CSRF) and DNS rebinding.** Any state-changing request
(POST/PATCH/DELETE) must include `X-Karucode-Client: web`. A request that carries
an `Origin` header must have an origin whose authority matches the `Host` header
(case-insensitive; values containing whitespace or control characters are
rejected outright). Simple cross-site form submissions and most cross-origin
`fetch` calls are rejected with 403. This is necessary because the API is
reachable from `http://127.0.0.1:<port>` and a malicious page in your browser can
attempt requests to that origin.

Comparing `Origin` with `Host` does not stop **DNS rebinding** (`evil.example`
re-pointed at `127.0.0.1` sends a matching `Origin` *and* `Host`), so when the
server is bound to a loopback address and no `KARUCODE_AUTH_TOKEN` is configured
it additionally serves only `Host: 127.0.0.1`, `localhost` or `[::1]` (any port),
plus whatever `KARUCODE_ALLOWED_HOSTS` lists for a reverse proxy
(`host`, `host:port`, or `*.example.com` for subdomains). Anything else is a
`403 permission_denied` naming the accepted hosts. With a token configured the
`Host` check is skipped, because the token is the credential.

**Git panels (`GET /api/git/status`, `GET /api/git/diff`).** `git` is spawned
with an argv array (never a shell), with `-c core.fsmonitor=false`,
`-c diff.external=`, `-c core.pager=cat`, `-c core.attributesFile=/dev/null`,
`--no-ext-diff` and `--no-textconv` for diffs, and
`GIT_EXTERNAL_DIFF`/`GIT_PAGER`/`GIT_OPTIONAL_LOCKS`/`GIT_TERMINAL_PROMPT` pinned,
`GIT_CONFIG_NOSYSTEM`/`GIT_ATTR_NOSYSTEM` set, and inherited
`GIT_DIR`/`GIT_WORK_TREE`/`GIT_CONFIG_COUNT` removed.

`--no-ext-diff` and `--no-textconv` do **not** cover `filter.<driver>.clean`,
`smudge` or `process`, which `git diff` runs to compute the working-tree side of a
diff. Those are neutralised explicitly: every driver named in the repository's
config is overridden to an empty command through `GIT_CONFIG_KEY_n`/`VALUE_n`
(not `-c`, whose first-`=` split a driver name containing `=` can bypass), and
`filter.<driver>.required` is forced to `false` so a neutralised diff does not
fail. The HTTP route and the tool share one argv builder
(`kc_tools::git::diff_args`), so the two cannot drift apart again. A cloned
repository therefore cannot make these read-only endpoints execute code through
its own `.git/config` or `.gitattributes`.

**Project roots.** `POST /api/projects` accepts any existing directory, except
`/`. A project's directory becomes the confinement root for every file tool in
its sessions, and the default posture is a single operator on a loopback bind
whose agent runs as the same user as the server — so an allowlist that left the
default workspace as the only legal project was a limitation with no security
value: the obvious second project is a sibling checkout. Set
`project_roots` (`KARUCODE_PROJECT_ROOTS`, colon-separated) to turn project
creation into an allowlist, compared component-wise on canonical paths, for a
shared or remotely reachable deployment. `/` is refused either way: one request
must not be able to make the whole filesystem the confinement root, which would
make the confinement rules below meaningless.

**Admin gate.** With `KARUCODE_ADMIN_TOKEN` set, a `PATCH /api/config` that
changes `approvals`, `model.base_url`, `model.api_key`, `providers`,
`active_provider` or `active_model` must also present
`X-Karucode-Admin-Token` (constant-time compare, `403` otherwise). Without it,
any holder of the ordinary token could turn approvals off and then repoint the
provider at a host of its choosing. Every authorised change is logged with the
field names that changed — never the values.

**File tool confinement.** `read_file`, `list_dir`, `search_files`, `write_file`,
`edit_file` and the workspace explorer resolve paths through `kc_core::Workspace`:

1. `..` segments may not climb above the workspace root (lexical check);
2. absolute paths outside the root are refused;
3. existing paths are canonicalised, so a symlink pointing outside the workspace
   is refused;
4. for writes, the deepest existing ancestor is canonicalised before the file is
   created, so a symlinked directory cannot be used to escape;
5. NUL bytes are rejected.

Writes are atomic (temp file + rename) and carry a size cap.

These are *checks, not handles* (`kc_core::paths` says so at the top of the
module). Resolution returns a path and the caller then opens or writes it by name,
so a component swapped for a symlink between the check and the syscall defeats
rule 4. Closing that window needs `openat2(RESOLVE_BENEATH)` or passed handles
(`cap-std`), not a second `stat`. It is not treated as a vulnerability of its own
for the same reason `/tmp` is inside the fence: the agent can already run
`run_command` as this user, so anything that can win the race can read the file
directly. What these rules do prevent is a *model* walking out of its project with
a plain `../` or a committed symlink.

**A workspace has more than one root.** Besides the project directory, every
workspace allows the system temporary directory (`TMPDIR` when set and absolute,
otherwise `/tmp`) — `allow_temp_dir`, on by default, off with
`KARUCODE_ALLOW_TEMP_DIR=0`. Every rule above is applied to the *set* of roots: a
path is allowed when it is inside the project **or** the temp dir, `..` is
measured from the root the path belongs to (`/tmp/../etc/passwd` is refused), and
a symlink inside the temp dir that points elsewhere is refused. Only `/` is
refused as an extra root — it would make every absolute path legal. A workspace
that itself lives under `/tmp` (a scratch or CI checkout) keeps the temp root.

This is not a weakening. `/tmp` is writable by every process on the host, and
`run_command` is unsandboxed and runs as the server user, so a rule that stopped
`write_file` reaching `/tmp` would stop nothing that a shell could not already
do. What the confinement rules protect is the *model* leaving its project for
`~/.ssh`, `/etc` or another repository, and that still holds: those are not roots.

**Project roots.** `POST /api/projects` accepts any existing directory except
`/`; `project_roots` (empty by default) narrows that to an allowlist, compared
component-wise on canonical paths. `project_roots` governs where a project may be
*created*; `allow_temp_dir` governs what a run's tools may *touch*. They are
independent.

**Secrets.** The API key is read from the environment, the config file or the
settings table, and is never returned by the API: the browser only receives
`api_key_set: true`. Errors and logs redact credentials; provider errors are
truncated and stripped of the `Authorization` header. Plugin/MCP environment and
headers are never serialised into diagnostics.

Child processes do **not** inherit the server's environment. `run_command`,
plugins and stdio MCP servers each get a cleared environment plus a fixed
allowlist (`PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `LANG`, `LC_ALL`, `TERM`,
`TMPDIR`, `PWD`, `TZ`), and any name matching `KARUCODE_*`, `AWS_*`, `OPENAI_*`,
`ANTHROPIC_*`, `GITHUB_TOKEN`, `GH_TOKEN`, `NPM_TOKEN`, `*_API_KEY`, `*_TOKEN`,
`*_SECRET` or `*_PASSWORD` is dropped even if explicitly listed. Without this, a
prompt-injected `curl`, a malicious `postinstall` script or an `npx`-launched MCP
server would receive the harness bearer token and the provider API key.

This is not the same as "the agent cannot read the key". The agent runs as the
same OS user as the server, so its shell can read the config file and the SQLite
database directly. The only real mitigation is OS-level isolation (a container, a
separate user, or systemd `LoadCredential`); treat the key as readable by anything
the agent can run.

**Untrusted model output.** The UI renders Markdown with raw HTML disabled and
sanitises the result (DOMPurify). Links are marked `rel="noopener noreferrer"`.
Tool output, plugin output and MCP output are treated the same way — as data, not
as instructions.

## What is NOT protected — read this

**Shell commands are not sandboxed.** `run_command` starts `/bin/sh -c <command>`
as the server user. Workspace path confinement does **not** apply to shell
commands: a command can read `~/.ssh`, write outside the workspace, or reach the
network. The only gates are:

* the approval policy (`ask` by default) and the command allowlist;
* an explicit warning in the system prompt and in the UI;
* cancellation, timeouts and output limits (resource safety, not isolation).

If you need real isolation, run karucode as a dedicated unprivileged user inside a
container/VM with the project bind-mounted, and treat the approval policy as a
convenience rather than a security boundary.

**The packaged unit does not add a filesystem sandbox.** `packaging/karucode.service`
runs the agent as a dedicated unprivileged user with `NoNewPrivileges=yes`, but it
deliberately sets no `ProtectSystem=`, `ProtectHome=` or `ReadWritePaths=`: the
agent can write anywhere that user can, so `mv`, build caches and sibling
checkouts work instead of failing with `EROFS`. The **service account is the
boundary** — give it its own user and it cannot reach anything that user cannot.

That is a real reduction in containment compared with a read-only root, and it was
chosen deliberately: maintaining a `ReadWritePaths=` allowlist for every toolchain
was a permanent source of `EROFS` failures deep inside builds. If you want the wall
back, add it in `/etc/systemd/system/karucode.service.d/20-local.conf` (which no
tool rewrites) and run `systemctl daemon-reload && systemctl restart karucode`:

```ini
[Service]
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=/srv/karucode/workspace /var/lib/karucode
```

**Plugins and MCP servers are trusted code.** They run as separate processes with
the same OS permissions as the server. Process isolation means a crash or hang
cannot take the harness down, and the core can time out or kill them; it does not
prevent a malicious plugin from reading your files or your environment. Plugins
declare permissions in their manifest, and the core records them, but that is a
declaration for the user's benefit, not an enforced sandbox. Install plugins and
MCP servers you trust.

**Approval prompts are not a security boundary against a malicious model.**
A compromised or adversarial model can phrase a command innocuously. Always read
the command/path in the approval card before allowing it. `auto` mode and large
`allow_commands` lists remove that check; use them only in disposable
environments.

**Prompt injection.** The agent reads project files, command output and MCP
responses. A repository can contain text that tries to steer the model. The
system prompt tells the model to treat such content as data, and approvals gate
mutations, but prompt injection is not solved. Do not run an unattended agent
with `auto` approval on an untrusted repository.

**Workspace HTML previews.** Previewed workspace HTML executes script only in an
opaque sandbox. It cannot read application storage/cookies or issue script fetches,
and it cannot submit forms or embed remote assets. Only relative resources from
that workspace's own fixed preview route and data/blob resources load. A remote
URL or CDN is not network-authorized merely because a workspace page was opened.
Treat previewed files as untrusted despite the confinement; custom tool approval
and filesystem permissions are separate decisions.

**Chat artifacts.** HTML artifacts execute in opaque-origin sandboxed frames and
use a random capability that grants read access only to that artifact; they never
receive the deployment's master bearer token. In the app frame, script-initiated
network requests, remote subresources and form submissions are blocked. A directly
opened top-level artifact can still navigate itself to another URL, which can reveal
the viewing event and that artifact's capability (and therefore that document) to a
destination its script chooses. `Referrer-Policy: no-referrer` suppresses incidental
referrer leakage, but cannot stop script explicitly putting the capability in a new
URL. Keep secrets out of artifact source and share capability URLs only with people
allowed to read that document.

**Local multi-user hosts.** There is no per-user authorization beyond the optional
shared token. Any process that can read the token can use the agent. On a shared
machine, run it under your own account and keep the data directory private.

**Shared workspaces, including subagents.** Sessions and subagents in the same workspace see and change the same live files, including dirty and untracked parent state. Completed subagent edits are not rolled back by a separate-worktree cleanup. `owned_paths` serves as advisory conflict reporting (a warning in the subagent result and a row in the claims table), not file locking or isolation; use disjoint paths and coordinate overlapping work.

**Model provider.** Whatever you point `base_url` at receives your prompts, file
contents the agent reads, and command output. Use a gateway you trust, ideally on
a network path you control.

## Remote access recipe

If you must reach karucode from another machine:

```bash
KARUCODE_AUTH_TOKEN="$(openssl rand -hex 24)" \
KARUCODE_HOST=0.0.0.0 \
karucode
```

* Terminate TLS in front of it (Caddy/nginx/SSH tunnel). The server speaks plain
  HTTP.
* Prefer an SSH port-forward (`ssh -L 8787:127.0.0.1:8787 host`) over exposing the
  port: then no token needs to leave your machine.
* Set `KARUCODE_LOCK_SETTINGS=1` so provider settings cannot be changed from the
  browser.
* Keep the approval mode at `ask` and never add broad shell prefixes to
  `allow_commands`.

## Reporting

This is a local tool without a security response process. If you find an issue,
document reproduction steps and avoid running untrusted plugins or MCP servers
until it is fixed.