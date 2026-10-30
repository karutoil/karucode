#!/bin/sh
# Stand up a private kc-server + Chromium for scripts/verify_notifications.py.
#
# Nothing here is permanent: the temp dir, the ports and both processes belong to
# this run and are torn down on exit. `scripts/verify_notifications.py` still does
# its own setup/teardown of what it can reach over HTTP; this wrapper only starts
# the two processes it expects to already be running.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
BIN="$ROOT/target/debug/karucode"
TMP=$(mktemp -d /tmp/kc-notify-XXXXXX)
MOCK_PORT=$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')
PORT=$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')
CDP=$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')

mkdir -p "$TMP/workspace" "$TMP/data" "$TMP/profile"
printf '# demo\n' > "$TMP/workspace/README.md"

cleanup() {
  for pid in ${SERVER_PID:-} ${CHROME_PID:-}; do
    kill "$pid" 2>/dev/null || true
  done
  rm -rf "$TMP"
}
trap cleanup EXIT INT TERM

# A deterministic OpenAI-compatible endpoint: the checks need a real run to finish.
setsid nohup python3 "$ROOT/scripts/mock_openai.py" --port "$MOCK_PORT" \
  --log "$TMP/mock.log" >"$TMP/mock.out" 2>&1 </dev/null &
MOCK_PID=$!

i=0
while [ $i -lt 100 ]; do
  if python3 -c "import urllib.request,sys; urllib.request.urlopen('http://127.0.0.1:$MOCK_PORT/health', timeout=1)" 2>/dev/null; then
    break
  fi
  i=$((i + 1))
  sleep 0.2
done

cat > "$TMP/config.toml" <<EOF
host = "127.0.0.1"
log_level = "warn"

[model]
base_url = "http://127.0.0.1:$MOCK_PORT/v1"
provider = "openai_compatible"
model = "mock-coder"
api_key = "sk-mock"

[[providers]]
id = "default"
name = "Mock"
provider = "openai_compatible"
base_url = "http://127.0.0.1:$MOCK_PORT/v1"
api_key = "sk-mock"

[[providers.models]]
id = "mock-coder"
name = "Mock Coder"

[active]
provider = "default"
model = "mock-coder"
EOF

KARUCODE_WEB_DIR="$ROOT/web/dist" KARUCODE_LOG=warn \
  setsid nohup "$BIN" --port "$PORT" --workspace "$TMP/workspace" \
  --data-dir "$TMP/data" --config "$TMP/config.toml" \
  >"$TMP/server.log" 2>&1 </dev/null &
SERVER_PID=$!

i=0
while [ $i -lt 150 ]; do
  if python3 -c "import urllib.request,sys; urllib.request.urlopen('http://127.0.0.1:$PORT/api/health', timeout=1)" 2>/dev/null; then
    break
  fi
  i=$((i + 1))
  sleep 0.2
done

setsid nohup chromium --headless=new --no-sandbox --disable-gpu --disable-dev-shm-usage \
  --remote-debugging-port="$CDP" --user-data-dir="$TMP/profile" \
  --window-size=1400,900 about:blank >"$TMP/chrome.log" 2>&1 </dev/null &
CHROME_PID=$!

i=0
while [ $i -lt 150 ]; do
  if python3 -c "import urllib.request,sys; urllib.request.urlopen('http://127.0.0.1:$CDP/json/version', timeout=1)" 2>/dev/null; then
    break
  fi
  i=$((i + 1))
  sleep 0.2
done

echo "server :$PORT  cdp :$CDP  logs $TMP"
set +e
python3 "$ROOT/scripts/verify_notifications.py" --port "$PORT" --cdp "$CDP"
STATUS=$?
set -e
kill "$MOCK_PID" 2>/dev/null || true
exit $STATUS
