# Quick start

> Install karucode from the official installer, point it at an OpenAI-compatible endpoint, and give it one task — a few minutes on a Linux box that can already reach an API.

The release is a prebuilt binary with the browser UI already inside it, so there
is nothing to compile and nothing to deploy beside it. Rust and Node are only
needed to build karucode itself: see [CONTRIBUTING.md](../CONTRIBUTING.md).

## What you need

| | |
| --- | --- |
| An OS | Linux, `x86_64` — the target the release is built for |
| `curl` and `sh` | the installer is a POSIX shell script |
| systemd | for the service install; without it, pass `--no-systemd` |
| A model | any OpenAI-compatible endpoint: a gateway, a hosted API, or a model server you already run |

That is the whole list. There is no account, no signup and no telemetry: the
binary talks to the endpoint you give it and to nothing else.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/karutoil/karucode/main/packaging/install.sh | sudo sh
```

That is the service install: the binary, a hardened systemd unit and a drop-in
with this machine's paths, enabled at boot and started. Name the first project
with `--workspace`, and the flags go after `sh -s --`:

```bash
curl -fsSL https://raw.githubusercontent.com/karutoil/karucode/main/packaging/install.sh \
  | sudo sh -s -- --workspace /srv/karucode/workspace
```

No root, or no systemd? Install the binary only and run it yourself:

```bash
curl -fsSL https://raw.githubusercontent.com/karutoil/karucode/main/packaging/install.sh \
  | sh -s -- --prefix "$HOME/.local" --no-systemd
```

Prefer to read the script before it runs? That is the same file, saved and then
executed:

```bash
curl -fsSLO https://raw.githubusercontent.com/karutoil/karucode/main/packaging/install.sh
sudo sh install.sh --workspace /srv/karucode/workspace
```

Either way the release tarball is downloaded to a temporary file, verified
against the release's `SHA256SUMS`, and only then unpacked; a mismatch aborts
with nothing installed — the script is what gets piped, never the binary. The
previous binary is kept as `karucode.old`, and if the service fails to start or
`/api/health` never answers, the previous unit, drop-in and binary are restored
before the installer exits. `--dry-run` prints every action and changes nothing.

## First run

Open <http://127.0.0.1:8787>. The first-run wizard takes four things and nothing
else: the model endpoint (with a live *Test connection*), appearance, the
approval mode, and a password or passkey for the web UI.

The endpoint is a prefix, not a full URL — karucode appends `/chat/completions`
to it, so point it at the `/v1` root of your gateway. A sealed deployment can set
the same three values in the environment instead:

```bash
export KARUCODE_BASE_URL=http://127.0.0.1:4000/v1   # your gateway or provider
export KARUCODE_API_KEY=sk-...                     # if the gateway needs one
export KARUCODE_MODEL=gpt-4o-mini
```

`OPENAI_BASE_URL` and `OPENAI_API_KEY` work as fallbacks. The full precedence
order — flags, environment, config file, database — is
[Change karucode's configuration](internal/configuration.md); named provider
profiles and the model catalogue are
[Configure a model provider](internal/provider-profiles.md).

## Point it at a provider

Anything that speaks the OpenAI chat-completions API works; the value you give it
is the API root, not a single endpoint. Tested shapes:

| Server | `KARUCODE_BASE_URL` |
| --- | --- |
| LiteLLM proxy | `http://127.0.0.1:4000/v1` |
| vLLM | `http://127.0.0.1:8000/v1` |
| Ollama | `http://127.0.0.1:11434/v1` |
| llama.cpp server | `http://127.0.0.1:8080/v1` |
| OpenRouter | `https://openrouter.ai/api/v1` |

With Ollama, tool calling depends on the model you pull: a model that cannot emit
tool calls cannot drive the agent. Providers are install-wide, not per project,
although a session can pin its own model, provider and reasoning effort.

## Give it a task

Create a project (a directory), open a session in it, and type what you want. It
reads the files, searches them, edits them, runs commands with streamed output
and inspects git, and it keeps going until the work is done or you stop it.

The default approval mode is `ask`: shell commands and file writes wait for you,
and you answer allow-once, allow-for-session or deny. Every decision is recorded.
`auto`, `ask_commands` and `read_only` are the other modes — see
[Control what the agent may do without asking](internal/approvals.md).

To try it on a real bug, run it against the example project in the repository,
whose `add()` subtracts and whose test suite therefore fails:

```bash
karucode --workspace examples/demo-app
# prompt: Run the tests, find the failing cases, fix the bug in src/math.js,
#         and re-run the tests to prove it passes.
```

## Installer flags

Every flag is optional; `sudo sh install.sh` and the one-liner above are the
whole install. `sh install.sh --help` prints the same list.

| Flag | What it does |
| --- | --- |
| `--workspace DIR` | create this directory and use it as the first project. Without it the server starts with no project and you add one in the UI |
| `--version VER` | release to install instead of the newest build, e.g. `--version 107` |
| `--prefix DIR` | install the binary to `<DIR>/bin`. Default `/usr/local` |
| `--data-dir DIR` | SQLite database and plugin data. Default: the account's own home for a system account (e.g. `/var/lib/karucode`), else `~/.local/share/karucode` |
| `--host ADDR` | bind address. Default `127.0.0.1` |
| `--port PORT` | listen port. Default `8787` |
| `--service-user NAME` | run the service as this account. Default: whoever ran the installer. Never root, never created |
| `--no-systemd` | install the binary only: no user, no unit, no `systemctl` |
| `--dry-run` | print every action and change nothing |

## Versions and upgrades

There are no semantic releases: **every commit on `main` is a release and the
version is the commit count**, so build 107 is the 107th commit.

```bash
$ karucode --version
karucode 107 (abf0a4216839, 2026-11-09T21:35:10-05:00)
```

Re-running the installer is the upgrade. A binary it installed also updates
itself from the release manifest:

```bash
karucode update --check        # exit 0: up to date, exit 3: a newer build exists
sudo karucode update --apply --restart
```

The download is verified against the manifest's SHA-256 and executed once with
`--version` before it is installed, and the old binary is kept. Settings →
Updates shows the running build and the release notes, or the exact `sudo`
command when the service account cannot write the binary. Details:
[Update karucode automatically](internal/updates.md) and
[Understand karucode's version and build number](internal/versioning.md).

## Before you expose it

The server binds `127.0.0.1` and refuses a non-loopback address unless
`KARUCODE_AUTH_TOKEN` (16 characters or more) is set, or web sign-in is
configured. There is no TLS in the binary: put it behind a reverse proxy or an
SSH tunnel. Set `KARUCODE_ADMIN_TOKEN` as well, so an ordinary API client cannot
turn approvals off or repoint the provider.

The whole posture — what is protected, what explicitly is not, and the recipe for
safe remote access — is [Security model and trust boundaries](../SECURITY.md).

## Where to go next

| If you want to | Read |
| --- | --- |
| change how it behaves, layer config, understand precedence | [Change karucode's configuration](internal/configuration.md) |
| point it at a gateway, a local model server, or several providers | [Configure a model provider](internal/provider-profiles.md) |
| run it as a service, move the port, find the database | [Run it as a systemd service](internal/deployment.md) |
| know every CLI flag and startup failure | [Build, run and stop the server](internal/run-server.md) |
| script it, or call it from another program | [Call the HTTP API](internal/http-api.md) |
| add tools of your own | [Write and install a plugin](internal/plugins.md) |
| use the browser workspace day to day | [Use the browser workspace](internal/web-ui.md) |