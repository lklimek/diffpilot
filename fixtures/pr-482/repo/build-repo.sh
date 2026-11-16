#!/usr/bin/env bash
# Builds the example repository of acme/node-client#482 from base/ and head/ next to this script
# (Rust sources there are stored as *.rs.txt and get their .rs name back):
# branch `main` = base commit, branch `feat/retry-timeouts` and `refs/pull/482/head` = head commit.
# Commit ids are reproducible (fixed identities and dates, user and system git config ignored);
# they are recorded in commits.json.
#
# Usage: build-repo.sh DEST   (DEST must not exist or be empty)
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dest="${1:?usage: build-repo.sh DEST}"

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME="Dana Developer" GIT_AUTHOR_EMAIL="dana@example.com"
export GIT_COMMITTER_NAME="Dana Developer" GIT_COMMITTER_EMAIL="dana@example.com"

mkdir -p "$dest"
git init -q -b main "$dest"
commit() {
  # commit TREE_DIR DATE MESSAGE: replaces the work tree with TREE_DIR and commits it.
  git -C "$dest" rm -q -r --ignore-unmatch . > /dev/null
  cp -R "$1/." "$dest/"
  # Rust sources are stored as *.rs.txt (keeps rustfmt off the fixture layout).
  find "$dest" -path "$dest/.git" -prune -o -type f -name '*.rs.txt' -print0 |
    while IFS= read -r -d '' f; do mv "$f" "${f%.txt}"; done
  git -C "$dest" add -A
  GIT_AUTHOR_DATE="$2" GIT_COMMITTER_DATE="$2" git -C "$dest" -c commit.gpgsign=false commit -q -m "$3"
}
commit "$here/base" "2026-11-02T09:00:00+00:00" "chore: node-client 0.7.0"
git -C "$dest" checkout -q -b feat/retry-timeouts
commit "$here/head" "2026-11-10T14:30:00+00:00" "feat(rpc): retry with backoff on timeout"
git -C "$dest" update-ref refs/pull/482/head HEAD
echo "base $(git -C "$dest" rev-parse main)"
echo "head $(git -C "$dest" rev-parse HEAD)"
