#!/usr/bin/env bash
# Development stack of smart-review on localhost: fake GitHub, server (embedded web UI), runner (fake provider).
# Runs in the foreground; Ctrl+C or SIGTERM stops every component. Fake credentials only: generated per state
# dir or taken from the fixture recordings.
#
# Usage: deploy/dev/stack.sh [--without COMPONENT]...     COMPONENT: runner | fake-github
#
# Inputs (all optional):
#   SR_DEV_STATE_DIR        state and logs (default: new dir /data/tmp/smart-review-dev.XXXXXX; relative paths
#                           are resolved against the working directory); kept on exit;
#                           reusing it keeps the database, the clone base and the generated secrets
#   SR_DEV_PORT             public port of the server (default: a free port)
#   SR_DEV_SERVER_BIN       prebuilt server with feature embed-ui; with the two below, nothing is built
#   SR_DEV_RUNNER_BIN       prebuilt runner with feature fake-provider
#   SR_DEV_FAKE_GITHUB_BIN  prebuilt tools/fake-github
#   RUST_LOG                log filter of all components (default: info)
#   SR_DEV_GITHUB_RECORDINGS  recordings fake GitHub serves (default: fixtures/pr-482/github)
#   SR_DEV_CLONE_URL        clone base (default: the fixture remote built into <state>/remotes)
#   SR_DEV_RUNNER_LISTEN_ADDR  runner listener (default: 127.0.0.1:<free port>); for a runner in a container
#
# Environment set for the components (each starts from an empty environment plus these; the fixture clone
# base build gets the `all` set):
#   all          PATH (inherited), HOME=<state>/home (empty, isolates git config), LANG=C.UTF-8,
#                NO_COLOR=1 (plain-text logs), RUST_LOG
#   fake-github  FAKE_GITHUB_LISTEN        127.0.0.1:<free port>
#                FAKE_GITHUB_FIXTURE_DIR   = SR_DEV_GITHUB_RECORDINGS
#                FAKE_GITHUB_APP_URL       = SR_PUBLIC_URL (callback <url>/auth/github/callback)
#   server       SR_PUBLIC_URL             http://127.0.0.1:<public port> (plain HTTP: cookies not Secure)
#                SR_LISTEN_ADDR            127.0.0.1:<public port>
#                SR_RUNNER_LISTEN_ADDR     = SR_DEV_RUNNER_LISTEN_ADDR (runner protocol only)
#                SR_DATA_DIR               <state>/server (SQLite database, bare mirrors)
#                SR_SECRET_KEY             random 32 bytes, base64 (token encryption at rest)
#                SR_RUNNER_SECRET          random hex, shared with the runner
#                SR_GITHUB_APP_ID          1 (fake; app tokens are not minted in stage 2)
#                SR_GITHUB_CLIENT_ID       fixture-app-client-id   (fixtures/pr-482/github/oauth/access-token.json)
#                SR_GITHUB_CLIENT_SECRET   fixture-client-secret   (same recording)
#                SR_GITHUB_APP_PRIVATE_KEY_FILE  <state>/github-app-key.pem (placeholder, not a key; unread in stage 2)
#                SR_GITHUB_API_URL         http://127.0.0.1:<fake-github port>
#                SR_GITHUB_OAUTH_URL       http://127.0.0.1:<fake-github port>
#                SR_GITHUB_CLONE_URL       = SR_DEV_CLONE_URL, default file://<state>/remotes
#                                          (tools/fake-github/prepare-clone-base.sh)
#                SR_MEMCAN_URL             http://127.0.0.1:<free port>/mcp (nothing listens: no fake MemCan yet)
#                SR_MEMCAN_TOKEN           fake-memcan-token
#   runner       SR_RUNNER_SERVER_URL      http://<SR_RUNNER_LISTEN_ADDR>  (not started with --without runner)
#                SR_RUNNER_SECRET          = the server's
#                SR_RUNNER_ID              dev-runner
#                SR_RUNNER_STATE_DIR       <state>/runner
#                SR_RUNNER_PROVIDER        fake (fixture replay; ignores SR_RUNNER_CREDENTIAL_ID, default "default")
#
# The sign-in allowlist gets the fake user `alice` (stage 2 has no allowlist settings route): inserted into
# the database once the server has created it. Output: one line `SR_DEV_URL=<url>` when the stack is ready, then
# `SR_DEV_STATE_DIR=<dir>`; the generated values are in <state>/stack.env (mode 600) for the tests' log scan.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
login=alice
without=()
while (($#)); do
  case "$1" in
    --without)
      case "${2:-}" in
        runner | fake-github) without+=("$2") ;;
        *) echo "stack.sh: --without takes runner or fake-github" >&2; exit 2 ;;
      esac
      shift 2
      ;;
    -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) echo "stack.sh: unknown argument $1 (see --help)" >&2; exit 2 ;;
  esac
done
skipped() { [[ " ${without[*]} " == *" $1 "* ]]; }

for tool in curl git python3 sqlite3; do
  command -v "$tool" > /dev/null || { echo "stack.sh: $tool is required" >&2; exit 1; }
done

pids=()
# shellcheck disable=SC2317  # invoked by the EXIT trap
stop() {
  trap - EXIT INT TERM
  local pid
  # Each component runs in its own process group (setsid): the whole group gets the signal.
  for pid in "${pids[@]}"; do kill -TERM -- "-$pid" 2> /dev/null || true; done
  for _ in $(seq 50); do
    local alive=0
    for pid in "${pids[@]}"; do kill -0 -- "-$pid" 2> /dev/null && alive=1; done
    ((alive)) || break
    sleep 0.2
  done
  for pid in "${pids[@]}"; do kill -KILL -- "-$pid" 2> /dev/null || true; done
  echo "smart-review dev stack stopped; logs in ${state:-?}/logs" >&2
}
trap stop EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# --- binaries -------------------------------------------------------------------------------------------------
if [[ -n "${SR_DEV_SERVER_BIN:-}" && -n "${SR_DEV_RUNNER_BIN:-}" && -n "${SR_DEV_FAKE_GITHUB_BIN:-}" ]]; then
  server_bin="$SR_DEV_SERVER_BIN" runner_bin="$SR_DEV_RUNNER_BIN" fake_github_bin="$SR_DEV_FAKE_GITHUB_BIN"
else
  echo "building the web UI and the binaries (set SR_DEV_*_BIN to skip)" >&2
  [[ -d "$repo/web/node_modules" ]] || npm --prefix "$repo/web" ci >&2
  npm --prefix "$repo/web" run build >&2
  built="$(cd "$repo" && cargo build --message-format=json-render-diagnostics \
    -p server --features server/embed-ui -p runner --features runner/fake-provider -p fake-github)"
  executable() {
    python3 -c 'import json, sys
for line in sys.stdin:
    try:
        msg = json.loads(line)
    except ValueError:
        continue
    if msg.get("reason") == "compiler-artifact" and msg["target"]["name"] == sys.argv[1] and msg.get("executable"):
        print(msg["executable"])' "$1" <<< "$built"
  }
  server_bin="$(executable smart-review-server)" runner_bin="$(executable smart-review-runner)"
  fake_github_bin="$(executable fake-github)"
fi
for bin in "$server_bin" "$runner_bin" "$fake_github_bin"; do
  [[ -x "$bin" ]] || { echo "stack.sh: not an executable: $bin" >&2; exit 1; }
done

# --- state, ports, generated credentials ----------------------------------------------------------------------
mkdir -p /data/tmp
state="${SR_DEV_STATE_DIR:-$(mktemp -d /data/tmp/smart-review-dev.XXXXXX)}"
# Components start in other working directories: the state dir must be absolute.
state="$(mkdir -p "$state" && cd "$state" && pwd)"
mkdir -p "$state/logs" "$state/home" "$state/server" "$state/runner"
chmod 700 "$state"
read -r free_public github_port runner_port memcan_port < <(python3 -c 'import socket
socks = [socket.socket() for _ in range(4)]
for s in socks:
    s.bind(("127.0.0.1", 0))
print(*(s.getsockname()[1] for s in socks))')
public_port="${SR_DEV_PORT:-$free_public}"
runner_addr="${SR_DEV_RUNNER_LISTEN_ADDR:-127.0.0.1:$runner_port}"
recordings="${SR_DEV_GITHUB_RECORDINGS:-$repo/fixtures/pr-482/github}"
url="http://127.0.0.1:$public_port"
github="http://127.0.0.1:$github_port"
# A reused state dir keeps its secrets: the database holds tokens encrypted with SR_SECRET_KEY.
secret_key="$(sed -n 's/^SR_SECRET_KEY=//p' "$state/stack.env" 2> /dev/null || true)"
runner_secret="$(sed -n 's/^SR_RUNNER_SECRET=//p' "$state/stack.env" 2> /dev/null | head -n 1 || true)"
[[ -n "$secret_key" ]] || secret_key="$(head -c 32 /dev/urandom | base64)"
[[ -n "$runner_secret" ]] || runner_secret="$(head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n')"
echo "placeholder: stage 2 mints no GitHub App tokens" > "$state/github-app-key.pem"

common=(PATH="$PATH" HOME="$state/home" LANG=C.UTF-8 NO_COLOR=1 RUST_LOG="${RUST_LOG:-info}")
# Empty environment: inherited GIT_DIR, GIT_WORK_TREE or GIT_INDEX_FILE would redirect the fixture's git commands.
[[ -n "${SR_DEV_CLONE_URL:-}" || -d "$state/remotes/acme/node-client.git" ]] ||
  env -i "${common[@]}" "$repo/tools/fake-github/prepare-clone-base.sh" "$state/remotes" > /dev/null
server_env=(
  SR_PUBLIC_URL="$url"
  SR_LISTEN_ADDR="127.0.0.1:$public_port"
  SR_RUNNER_LISTEN_ADDR="$runner_addr"
  SR_DATA_DIR="$state/server"
  SR_SECRET_KEY="$secret_key"
  SR_RUNNER_SECRET="$runner_secret"
  SR_GITHUB_APP_ID=1
  SR_GITHUB_CLIENT_ID=fixture-app-client-id
  SR_GITHUB_CLIENT_SECRET=fixture-client-secret
  SR_GITHUB_APP_PRIVATE_KEY_FILE="$state/github-app-key.pem"
  SR_GITHUB_API_URL="$github"
  SR_GITHUB_OAUTH_URL="$github"
  SR_GITHUB_CLONE_URL="${SR_DEV_CLONE_URL:-file://$state/remotes}"
  SR_MEMCAN_URL="http://127.0.0.1:$memcan_port/mcp"
  SR_MEMCAN_TOKEN=fake-memcan-token
)
runner_env=(
  SR_RUNNER_SERVER_URL="http://$runner_addr"
  SR_RUNNER_SECRET="$runner_secret"
  SR_RUNNER_ID=dev-runner
  SR_RUNNER_STATE_DIR="$state/runner"
  SR_RUNNER_PROVIDER=fake
)
(umask 077 && printf '%s\n' "${server_env[@]}" "${runner_env[@]}" > "$state/stack.env")

# start NAME DIR COMMAND...: runs COMMAND in DIR in a new session (process group), output to logs/NAME.log.
start() {
  local name="$1" dir="$2"
  shift 2
  (cd "$dir" && exec setsid env -i "${common[@]}" "$@" > "$state/logs/$name.log" 2>&1 < /dev/null) &
  pids+=("$!")
}
# wait_for NAME URL: waits until URL answers with any HTTP status, or fails naming the log.
wait_for() {
  for _ in $(seq 150); do
    curl -s -o /dev/null --max-time 1 "$2" && return 0
    sleep 0.2
  done
  echo "stack.sh: $1 did not start; see $state/logs/$1.log" >&2
  exit 1
}

if ! skipped fake-github; then
  start fake-github "$state" FAKE_GITHUB_LISTEN="127.0.0.1:$github_port" \
    FAKE_GITHUB_FIXTURE_DIR="$recordings" FAKE_GITHUB_APP_URL="$url" "$fake_github_bin"
  wait_for fake-github "$github/user"
fi
# The server reads a `.env` of its working directory or a parent: run it in the state dir.
start server "$state" "${server_env[@]}" "$server_bin"
wait_for server "$url/api/v1/me"
sqlite3 -cmd ".timeout 5000" "$state/server/smart-review.sqlite" \
  "INSERT OR IGNORE INTO tenant_allowed_logins (tenant_id, login) VALUES ('default', '$login');"
if ! skipped runner; then
  start runner "$state" "${runner_env[@]}" "$runner_bin"
fi

echo "SR_DEV_URL=$url"
echo "SR_DEV_STATE_DIR=$state"
echo "smart-review dev stack ready at $url (sign in as $login; fixture PR https://github.com/acme/node-client/pull/482)" >&2
echo "skipped: ${without[*]:-none}; logs: $state/logs; Ctrl+C stops the stack" >&2
# Stop the stack when any component exits.
wait -n
echo "stack.sh: a component exited; see $state/logs" >&2
exit 1
