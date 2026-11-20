#!/usr/bin/env bash
# Docker image tags of a release (.github/workflows/release-images.yml): prints X.Y.Z, then X.Y, X and latest, each
# only when VERSION is the highest release in its line among EXISTING (git tags vX.Y.Z; other tags are ignored), so
# a patch release of an older line never moves the newer line's tags back.
#
# Usage:
#   scripts/release-image-tags.sh VERSION [EXISTING_TAG...]   VERSION is X.Y.Z (no v prefix, no pre-release)
#   scripts/release-image-tags.sh --self-test
set -euo pipefail

semver='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'

# highest PREFIX VERSION...: the highest version starting with PREFIX (or any, when PREFIX is empty).
highest() {
  local prefix="$1"
  shift
  printf '%s\n' "$@" | grep -E "^${prefix//./\\.}" | sort -t . -k 1,1n -k 2,2n -k 3,3n | tail -n 1
}

tags() {
  local version="$1" major minor
  shift
  if [[ ! "$version" =~ $semver ]]; then
    echo "release-image-tags: '$version' is not a release version X.Y.Z" >&2
    return 2
  fi
  major="${BASH_REMATCH[1]}" minor="${BASH_REMATCH[2]}"
  local versions=("$version") tag
  for tag in "$@"; do
    [[ "${tag#v}" != "$tag" && "${tag#v}" =~ $semver ]] && versions+=("${tag#v}")
  done
  echo "$version"
  [[ "$(highest "$major.$minor." "${versions[@]}")" == "$version" ]] && echo "$major.$minor"
  [[ "$(highest "$major." "${versions[@]}")" == "$version" ]] && echo "$major"
  [[ "$(highest "" "${versions[@]}")" == "$version" ]] && echo latest
  return 0
}

self_test() {
  local failed=0 got
  # expect WANT ARGS...: WANT is the space-separated output, or "exit N".
  expect() {
    local want="$1" status=0
    shift
    got="$(tags "$@" 2> /dev/null | tr '\n' ' ')" || status=$?
    got="${got% }"
    if ((status != 0)); then got="exit $status"; fi
    if [[ "$got" != "$want" ]]; then
      echo "release-image-tags self-test FAILED: $* -> '$got', want '$want'" >&2
      failed=1
    fi
  }
  expect "1.2.0 1.2 1 latest" 1.2.0 v1.0.0 v1.1.0 v1.2.0
  expect "1.2.0 1.2 1 latest" 1.2.0
  expect "1.1.5 1.1" 1.1.5 v1.1.4 v1.2.0 v1.1.5
  expect "1.1.5" 1.1.5 v1.1.6
  expect "0.3.0 0.3 0" 0.3.0 v1.0.0
  expect "1.10.0 1.10 1 latest" 1.10.0 v1.9.9 v1.2.0
  expect "1.9.1 1.9" 1.9.1 v1.10.0
  expect "2.0.0 2.0 2 latest" 2.0.0 v10.0.0-rc.1 v3.0.0rc foo v2.0.0
  expect "1.2.3 1.2 1" 1.2.3 v11.0.0 v1.2.3
  expect "exit 2" v1.2.0
  expect "exit 2" 1.2
  expect "exit 2" 1.2.0-rc.1
  expect "exit 2" 01.2.0
  if ((failed)); then return 1; fi
  echo "release-image-tags self-test: ok"
}

if [[ "${1:-}" == --self-test ]]; then
  self_test
else
  [[ $# -ge 1 ]] || { sed -n '2,8p' "$0" >&2; exit 2; }
  tags "$@"
fi
