#!/usr/bin/env bash
# Prepares a clone base for the dev stack: DEST/acme/node-client.git, a bare mirror of the fixture repository
# (fixtures/pr-482/repo/build-repo.sh) with all its refs, including refs/pull/482/head.
# Then set SR_GITHUB_CLONE_URL=file://DEST (the server clones <base>/<owner>/<name>.git).
#
# Usage: prepare-clone-base.sh DEST   (DEST/acme/node-client.git must not exist)
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dest="${1:?usage: prepare-clone-base.sh DEST}"

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
"$here/../../fixtures/pr-482/repo/build-repo.sh" "$work/repo" > /dev/null
mkdir -p "$dest/acme"
git clone -q --mirror "$work/repo" "$dest/acme/node-client.git"
echo "clone base ready: SR_GITHUB_CLONE_URL=file://$(cd "$dest" && pwd)"
