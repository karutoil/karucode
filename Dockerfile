# syntax=docker/dockerfile:1
#
# karucode - production container image.
#
# =============================================================================
# THIS CONTAINER IS NOT A SANDBOX FOR THE AGENT'S SHELL COMMANDS.
# =============================================================================
#
# karucode runs the commands the model asks for with `/bin/sh -c <command>` as the
# container user (`karucode`, uid 10001). A container is a packaging and
# blast-radius boundary, not a security boundary: the agent can read and write
# everything that user can reach inside this image, and it can reach the network
# unless you take that away at run time. See docs/SECURITY.md ("What is NOT
# protected") - the approval policy is a convenience, not isolation.
#
# What that means in practice:
#
#   * The agent can write anywhere the `karucode` user can write. The image is
#     read-only-ish only because the process is unprivileged; it is not enforced
#     by the image. If you need real containment, run this image with a read-only
#     root filesystem plus a writable bind mount for the workspace:
#         docker run --read-only --tmpfs /tmp \
#           -v /srv/projects:/workspace -v karucode-data:/data ...
#   * Mount ONLY the project directory you want the agent to touch. Do not mount
#     /, your home directory, or the Docker socket (`/var/run/docker.sock` would
#     hand the agent root on the host).
#   * Drop capabilities and keep the default seccomp profile:
#         --cap-drop=ALL --security-opt=no-new-privileges
#   * The agent needs `git` and `ripgrep` for its git/search tools; both are in
#     the image. Anything else it should use has to be installed by you.
#
# The /data volume holds the SQLite database (`karucode.db`, WAL mode) and the
# provider credentials: the API key is stored in the settings table, and the
# config file (if you create one) lives under $HOME/.config/karucode. Treat the
# volume as a secret: back it up encrypted, and do not share it between
# instances (the server takes an exclusive lock on <data_dir>/karucode.lock and
# refuses to start a second time on the same directory).
#
# Build:
#     docker build -t karucode:latest .
#
# Run (loopback only, no TLS in the binary - put a reverse proxy in front for
# anything remote, and set KARUCODE_AUTH_TOKEN when you do):
#     docker run -d --name karucode -p 127.0.0.1:8787:8787 \
#       -v karucode-data:/data -v /srv/projects:/workspace \
#       -e KARUCODE_AUTH_TOKEN="$(openssl rand -hex 32)" \
#       -e KARUCODE_BASE_URL=https://api.example.com/v1 \
#       -e KARUCODE_API_KEY=sk-... -e KARUCODE_MODEL=gpt-4o-mini \
#       karucode:latest
#
# Note: KARUCODE_HOST=0.0.0.0 is baked in below, and the server REFUSES to bind a
# non-loopback address without KARUCODE_AUTH_TOKEN (>= 16 chars). So the token is
# mandatory here, not optional - the container exits at startup without it.

# -----------------------------------------------------------------------------
# Stage 1: build the frontend bundle (web/dist).
#
# Node is a build-time dependency only. `web/.npmrc` sets `include=dev` and vite
# is a devDependency, so the full dependency tree is required - do not prune.
# -----------------------------------------------------------------------------
FROM node:22.12-bookworm-slim AS web

WORKDIR /src/web

# Copy the manifests first so `npm ci` is cached independently of source edits.
COPY web/package.json web/package-lock.json web/.npmrc ./
RUN npm ci

COPY web/ ./
RUN npm run build && test -f dist/index.html

# -----------------------------------------------------------------------------
# Stage 2: build the release binary.
#
# The server embeds the UI at compile time (rust-embed, feature `embed-ui`, on by
# default). `crates/kc-server/build.rs` reads KARUCODE_WEB_DIST and copies that
# directory over `web/dist` before embedding, so the bundle from stage 1 is what
# ends up inside the binary - no Node and no web/dist at run time.
#
# The image compiler is pinned to 1.96 so its builds do not drift with `stable`.
# The separately supported minimum is rust-version = "1.89" in Cargo.toml; CI's
# msrv job tests that floor, while this image deliberately uses a newer compiler.
# -----------------------------------------------------------------------------
FROM rust:1.96-bookworm AS build

WORKDIR /src

# `crates/kc-docs/build.rs` compiles the `@kc-doc` blocks out of `crates/`,
# `plugins/` and `web/src` at build time, so those trees must be present even
# though only kc-server is built.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/ ./crates/
COPY plugins/ ./plugins/
COPY web/ ./web/

# Replace the (absent) web/dist with the prebuilt bundle from stage 1.
COPY --from=web /src/web/dist ./web/dist

ENV KARUCODE_WEB_DIST=/src/web/dist

RUN cargo build --release --locked -p kc-server \
    && install -D -m 0755 target/release/karucode /out/karucode

# -----------------------------------------------------------------------------
# Stage 3: runtime.
#
# Only the binary is copied. `git` and `ripgrep` are the two external programs the
# built-in tools shell out to (kc-tools/src/git.rs, kc-tools/src/search.rs);
# `curl` is for the healthcheck. TLS is rustls (see Cargo.toml), so no libssl is
# needed - but ca-certificates is, for the provider HTTPS calls.
# -----------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        git \
        ripgrep \
    && rm -rf /var/lib/apt/lists/*

# Unprivileged, fixed uid/gid so bind-mounted workspaces can be chowned to match.
# Not `--system`: a system account is expected to have a uid below SYS_UID_MAX
# (999), and asking for 10001 there makes useradd warn on every build.
RUN groupadd --gid 10001 karucode \
    && useradd --uid 10001 --gid 10001 \
        --home-dir /data --no-create-home --shell /usr/sbin/nologin karucode \
    && install -d -o karucode -g karucode -m 0700 /data \
    && install -d -o karucode -g karucode -m 0755 /workspace

COPY --from=build /out/karucode /usr/local/bin/karucode

# KARUCODE_DATA_DIR: SQLite database + plugin data (the server forces 0700).
# KARUCODE_HOST: must be 0.0.0.0 to be reachable through the published port;
#                this makes KARUCODE_AUTH_TOKEN mandatory (see the note above).
# HOME: keeps `~/.config/karucode/config.toml` and git's config inside the volume.
ENV KARUCODE_DATA_DIR=/data \
    KARUCODE_HOST=0.0.0.0 \
    KARUCODE_WORKSPACE=/workspace \
    HOME=/data

VOLUME ["/data"]
EXPOSE 8787

USER karucode
WORKDIR /workspace

# GET /api/health answers before any model is configured, which is what makes it
# the right probe. It sits INSIDE the auth middleware, so when KARUCODE_AUTH_TOKEN
# is set the probe must send it or it gets a 401 and the container is marked
# unhealthy forever. The token is read from the environment at probe time, so it
# never appears in the image or in `docker inspect`.
#
# Podman note: the OCI image format has no HEALTHCHECK field, so podman drops this
# with a warning. Build with `podman build --format docker` if you want it kept.
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD ["sh", "-c", "if [ -n \"${KARUCODE_AUTH_TOKEN:-}\" ]; then set -- -H \"Authorization: Bearer ${KARUCODE_AUTH_TOKEN}\"; else set --; fi; curl -fsS -o /dev/null --max-time 4 \"$@\" http://127.0.0.1:8787/api/health"]

# SIGTERM is the default; the server drains gracefully (bounded at 10s) and
# checkpoints SQLite, so give it a moment before SIGKILL.
STOPSIGNAL SIGTERM

ENTRYPOINT ["/usr/local/bin/karucode"]
