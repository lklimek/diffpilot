#!/usr/bin/env bash
# Fails when a production deployment file enables a development or test switch (ADR-013 "Rust implementation",
# threat model T1/T4): the dev no-isolation mode, the supervisor's fault injection or unprivileged test mode,
# the prototype's fault switches, the fake LLM provider, or (production compose only) the tmpfs codex-homes switch.
# With docker compose available, the rendered production and local configurations are checked too.
#
# Usage: deploy/check-production-config.sh [--self-test]
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
forbidden='SR_RUNNER_DEV_NO_ISOLATION|SR_SUPERVISOR_TEST_FAULTS|SR_SUPERVISOR_UNPRIVILEGED|R1_FAULT_[A-Z_]+|fake-provider|SR_RUNNER_PROVIDER'
# Files that build or run the production stack (the local variant runs the same images).
files=(
  deploy/compose.yaml deploy/compose.local.yaml .env.example
  deploy/server/Dockerfile deploy/server/healthcheck.sh
  deploy/runner/Dockerfile deploy/runner/preflight.sh deploy/runner/strip-image.sh
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

# rendered ROOT: checks `docker compose config` of the production and the local stack with .env.example.
rendered() {
  local root="$1" out
  if ! docker compose version > /dev/null 2>&1; then
    echo "check-production-config: docker compose not available; rendered configuration not checked" >&2
    return 0
  fi
  # The private key path only has to exist for `config` when it is a bind source; it is not created.
  out="$(docker compose --env-file "$root/.env.example" -f "$root/deploy/compose.yaml" config 2>&1)" ||
    { echo "$out" >&2; return 1; }
  if grep -nE "$forbidden|$production_only" <<< "$out"; then return 1; fi
  out="$(docker compose --env-file "$root/.env.example" -f "$root/deploy/compose.yaml" \
    -f "$root/deploy/compose.local.yaml" config 2>&1)" || { echo "$out" >&2; return 1; }
  if grep -nE "$forbidden" <<< "$out"; then return 1; fi
  return 0
}

if [[ "${1:-}" == --self-test ]]; then
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
  echo "check-production-config: self-test passed"
  exit 0
fi

status=0
scan "$repo" || status=1
rendered "$repo" || status=1
if ((status)); then
  echo "check-production-config: a development or test switch is in a production file (lines above)" >&2
else
  echo "check-production-config: no development or test switch in the production files"
fi
exit "$status"
