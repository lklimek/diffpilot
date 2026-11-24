#!/usr/bin/env bash
# Render an isolated deployment with fake placeholders; no accounts, running stacks or volumes are accessed.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
log="${1:-/dev/null}"
tmp="$(mktemp -d /data/tmp/diffpilot-stack-config.XXXXXX)"
project="w9-stack-config-$$"
trap 'rm -rf "$tmp"' EXIT
sed -e 's|^SR_PUBLIC_URL=.*|SR_PUBLIC_URL=http://localhost:3000|' \
  -e 's|^SR_ALLOWED_LOGINS=.*|SR_ALLOWED_LOGINS=alice|' \
  -e 's|^SR_GITHUB_APP_ID=.*|SR_GITHUB_APP_ID=1|' \
  -e 's|^SR_GITHUB_CLIENT_ID=.*|SR_GITHUB_CLIENT_ID=fake|' \
  -e 's|^SR_GITHUB_APP_PRIVATE_KEY_PATH=.*|SR_GITHUB_APP_PRIVATE_KEY_PATH=/nonexistent/fake.pem|' \
  -e 's|^SR_MEMCAN_URL=.*|SR_MEMCAN_URL=http://memcan.invalid/mcp|' \
  "$repo/.env.example" > "$tmp/stack.env"
docker compose -p "$project" --env-file "$tmp/stack.env" -f "$repo/deploy/compose.yaml" \
  -f "$repo/deploy/compose.local.yaml" config --format json > "$tmp/config.json"
# shellcheck disable=SC2016 # jq program, no shell expansion
jq -e --arg project "$project" '
  .services as $s |
  ($s | keys | sort) == ["codex-gateway","egress-proxy","launcher","runner","server"] and
  $s.launcher.network_mode == "none" and
  ($s.launcher.volumes | any(.target == "/var/run/docker.sock")) and
  ([ $s | to_entries[] | select(.key != "launcher") | .value.volumes[]? |
     select(.target == "/var/run/docker.sock") ] | length == 0) and
  $s.launcher.environment.SR_CONTAINER_PREFIX == $project and
  $s.launcher.environment.SR_WORKER_NETWORK == ($project + "-worker") and
  $s.launcher.environment.SR_JUDGE_NETWORK == ($project + "-judge") and
  $s.runner.user == "10002:10002" and
  $s.runner.cap_drop == ["ALL"] and ($s.runner.cap_add // [] | length == 0) and
  $s.runner.environment.SR_RUNNER_MIRRORS_DIR == "/mirror" and
  ($s.runner.volumes | any(.target == "/mirror")) and
  ($s.runner.volumes | all(.target != "/work" and (.target | contains("codex-homes") | not) and (.source | contains("runner-state") | not))) and
  ($s.runner.environment | has("SR_RUNNER_CREDENTIAL_ID") | not) and
  ($s.launcher.environment.SR_WORKER_IMAGE | contains("@sha256:")) and
  ($s.launcher.environment.SR_FETCH_IMAGE | contains("@sha256:")) and
  (.networks.worker.internal and .networks.judge.internal) and
  ($s["codex-gateway"].networks | has("worker") and has("judge") and has("runner")) and
  ($s["egress-proxy"].networks | has("judge") | not)
' "$tmp/config.json" > /dev/null
printf '%s\n' 'CHECK container_runtime_config PASS launcher-only socket, private networks, pinned workers, no host login/checkout volumes' | tee -a "$log"
