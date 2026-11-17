#!/usr/bin/env bash
# Fails when a production deployment file enables a development or test switch (ADR-013 "Rust implementation",
# threat model T1/T4): the dev no-isolation mode, the supervisor's fault injection or unprivileged test mode,
# the prototype's fault switches, the fake LLM provider, or (production compose only) the tmpfs codex-homes switch.
# The rendered configurations (docker compose config) are checked too: production with the env file, and the local
# variant (compose.local.yaml) only with an SR_PUBLIC_URL on localhost. Without docker compose the check fails
# unless --no-render is given.
#
# Usage: deploy/check-production-config.sh [--env-file FILE [--local]] [--no-render] | --self-test [--no-render]
#   (no option)       the files, production rendered with .env.example, local rendered with it on localhost
#   --env-file FILE   production rendered with FILE (for example the deployment's .env)
#   --local           also the local variant rendered with FILE; fails unless FILE's SR_PUBLIC_URL is on localhost
#   --no-render       the files only (explicitly no rendering)
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
forbidden='SR_RUNNER_DEV_NO_ISOLATION|SR_RUNNER_DEV_HOST_GATEWAY|SR_SUPERVISOR_TEST_FAULTS|SR_SUPERVISOR_UNPRIVILEGED|R1_FAULT_[A-Z_]+|fake-provider|SR_RUNNER_PROVIDER'
# Files that build or run the production stack (the local variant runs the same images).
files=(
  deploy/compose.yaml deploy/compose.local.yaml .env.example
  deploy/server/Dockerfile deploy/server/healthcheck.sh
  deploy/runner/Dockerfile deploy/runner/strip-image.sh
  deploy/egress-proxy/Dockerfile deploy/egress-proxy/entrypoint.sh deploy/egress-proxy/squid.conf
)
# Production only: the local variant may put codex-homes on a tmpfs.
production_only='SR_CODEX_HOMES_ALLOW_TMPFS'
production_files=(deploy/compose.yaml .env.example deploy/runner/Dockerfile deploy/server/Dockerfile)

# scan ROOT: prints every offending line, returns 1 if there is one.
scan() {
  local root="$1" found=0 file
  for file in "${files[@]}"; do
    [[ -f "$root/$file" ]] || { echo "check-production-config: missing $file" >&2; found=1; continue; }
    if grep -nEH "$forbidden" "$root/$file"; then found=1; fi
  done
  for file in "${production_files[@]}"; do
    if grep -nEH "$production_only" "$root/$file"; then found=1; fi
  done
  return "$found"
}

env_file="" local=0 render=1 self_test=0
while (($#)); do
  case "$1" in
    --env-file) env_file="${2:?--env-file needs a file}"; shift 2 ;;
    --local) local=1; shift ;;
    --no-render) render=0; shift ;;
    --self-test) self_test=1; shift ;;
    *) echo "check-production-config: unknown argument $1 (see the header)" >&2; exit 2 ;;
  esac
done

# can_render: 0 when rendering is wanted and possible; 1 when --no-render; exits 1 when compose is missing.
can_render() {
  ((render)) || return 1
  docker compose version > /dev/null 2>&1 && return 0
  echo "check-production-config: docker compose is not available, so the rendered configuration cannot be checked; install it or pass --no-render" >&2
  exit 1
}

# rendered ROOT ENV_FILE: `docker compose config` of the production stack has no switch.
rendered() {
  local out
  # The private key path only has to exist for `up`; `config` does not check it.
  out="$(docker compose --env-file "$2" -f "$1/deploy/compose.yaml" config 2>&1)" || { echo "$out" >&2; return 1; }
  if grep -nE "$forbidden|$production_only" <<< "$out"; then return 1; fi
  return 0
}

# rendered_local ROOT ENV_FILE: the local variant has no switch but its tmpfs one, and serves localhost only.
rendered_local() {
  local out url
  out="$(docker compose --env-file "$2" -f "$1/deploy/compose.yaml" -f "$1/deploy/compose.local.yaml" config 2>&1)" ||
    { echo "$out" >&2; return 1; }
  if grep -nE "$forbidden" <<< "$out"; then return 1; fi
  url="$(sed -n 's/^ *SR_PUBLIC_URL: *//p' <<< "$out" | head -n 1)"
  if ! grep -Eq '^http://(localhost|127\.0\.0\.1|\[::1\])(:[0-9]+)?/?$' <<< "$url"; then
    echo "check-production-config: compose.local.yaml with SR_PUBLIC_URL=$url: the local variant (tmpfs codex-homes) is for localhost only" >&2
    return 1
  fi
  return 0
}

if ((self_test)); then
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  for file in "${files[@]}"; do
    mkdir -p "$tmp/$(dirname "$file")"
    cp "$repo/$file" "$tmp/$file"
  done
  scan "$tmp" > /dev/null || { echo "self-test: clean copy rejected" >&2; exit 1; }
  echo 'ENV SR_RUNNER_DEV_NO_ISOLATION=1' >> "$tmp/deploy/runner/Dockerfile"
  if scan "$tmp" > /dev/null; then echo "self-test: dev switch in the runner Dockerfile not caught" >&2; exit 1; fi
  cp "$repo/deploy/runner/Dockerfile" "$tmp/deploy/runner/Dockerfile"
  printf '      SR_CODEX_HOMES_ALLOW_TMPFS: "1"\n' >> "$tmp/deploy/compose.yaml"
  if scan "$tmp" > /dev/null; then echo "self-test: tmpfs switch in production compose not caught" >&2; exit 1; fi
  if can_render; then
    sed 's|^SR_PUBLIC_URL=.*|SR_PUBLIC_URL=http://localhost:3000|' "$repo/.env.example" > "$tmp/local.env"
    rendered_local "$repo" "$tmp/local.env" > /dev/null 2>&1 || { echo "self-test: localhost local variant rejected" >&2; exit 1; }
    if rendered_local "$repo" "$repo/.env.example" > /dev/null 2>&1; then
      echo "self-test: local variant with a public SR_PUBLIC_URL not caught" >&2
      exit 1
    fi
  fi
  echo "check-production-config: self-test passed"
  exit 0
fi

status=0
scan "$repo" || status=1
if can_render; then
  if [[ -n "$env_file" ]]; then
    rendered "$repo" "$env_file" || status=1
    if ((local)); then rendered_local "$repo" "$env_file" || status=1; fi
  else
    rendered "$repo" "$repo/.env.example" || status=1
    local_env="$(mktemp)"
    sed 's|^SR_PUBLIC_URL=.*|SR_PUBLIC_URL=http://localhost:3000|' "$repo/.env.example" > "$local_env"
    rendered_local "$repo" "$local_env" || status=1
    rm -f "$local_env"
  fi
fi
if ((status)); then
  echo "check-production-config: failed (lines above)" >&2
else
  echo "check-production-config: no development or test switch in the production files"
fi
exit "$status"
