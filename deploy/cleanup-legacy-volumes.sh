#!/usr/bin/env bash
# Owner-run after a successful v1.3 deploy: remove legacy credentials and scratch, printing names only.
# Usage: deploy/cleanup-legacy-volumes.sh COMPOSE_PROJECT --after-successful-deploy
set -euo pipefail
if (($# != 2)) || [[ "$2" != --after-successful-deploy || ! "$1" =~ ^[a-z0-9][a-z0-9_-]*$ ]]; then
  echo 'usage: cleanup-legacy-volumes.sh COMPOSE_PROJECT --after-successful-deploy' >&2
  exit 2
fi
project="$1"
existing="$(docker volume ls --format '{{.Name}}')"
volumes=()
for suffix in runner-state tmp-runner-codex-homes tmp-runner-work tmp-runner; do
  name="${project}_$suffix"
  if grep -qxF "$name" <<< "$existing"; then
    containers="$(docker ps -aq --filter "volume=$name")"
    if [[ -n "$containers" ]]; then
      echo "cleanup-legacy-volumes: $name is still attached to a container; remove the legacy container first" >&2
      exit 1
    fi
    volumes+=("$name")
  fi
done
# Docker also refuses removal if a container attaches after the preflight.
for name in "${volumes[@]}"; do docker volume rm "$name"; done
