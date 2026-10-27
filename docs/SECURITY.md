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

**Network exposure.** Loopback-only by default. With a token configured, every
request needs `Authorization: Bearer <token>`; the SSE endpoint additionally
accepts `?token=` because streams are harder to authenticate from some clients.
Without a token, anyone who can reach the port can use the agent.

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

**Secrets.** The API key is read from the environment, the config file or the
settings table, and is never returned by the API: the browser only receives
`api_key_set: true`. Errors and logs redact credentials; provider errors are
truncated and stripped of the `Authorization` header. Plugin/MCP environment and
headers are never serialised into diagnostics.

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

**Local multi-user hosts.** There is no per-user authorization beyond the optional
shared token. Any process that can read the token can use the agent. On a shared
machine, run it under your own account and keep the data directory private.

**Shared workspaces.** Sessions in the same workspace see each other's file
changes. Subagents share the same workspace; `owned_paths` is advisory conflict
reporting (a warning in the subagent result and a row in the claims table), not
file locking or isolation. There is no per-subagent worktree.

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