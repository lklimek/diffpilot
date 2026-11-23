#!/usr/bin/env bash
# Builds the spike Codex image diffpilot-spike-codex:<version> with the Codex pins of the runner image
# (CODEX_VERSION and CODEX_PACKAGE_SHA256 in deploy/runner/Dockerfile) and prints its Codex version.
# Usage: deploy/dev/spike-codex/build.sh      Heavy job (Docker build). Needs no credential and creates no volume.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
runner_dockerfile="${here}/../../runner/Dockerfile"

# pin NAME: the value of the single `ARG NAME=<value>` line of the runner Dockerfile.
pin() {
  local values
  values="$(sed -n "s/^ARG $1=\([0-9a-f.]*\)\$/\1/p" "${runner_dockerfile}")"
  if [[ -z "${values}" || "${values}" == *$'\n'* ]]; then
    echo "build.sh: expected one ARG $1 in ${runner_dockerfile}" >&2
    exit 1
  fi
  printf '%s' "${values}"
}

version="$(pin CODEX_VERSION)"
sha256="$(pin CODEX_PACKAGE_SHA256)"
image="diffpilot-spike-codex:${version}"
docker build -t "${image}" --build-arg "CODEX_VERSION=${version}" --build-arg "CODEX_PACKAGE_SHA256=${sha256}" "${here}"
docker run --rm --network none "${image}" codex --version
echo "build.sh: built ${image}"
