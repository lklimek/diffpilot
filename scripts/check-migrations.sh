#!/usr/bin/env bash
# Migrations are append-only: a database already holds every applied migration's checksum and the server refuses to
# start when a file it applied has changed. Fails when a migration in crates/db/migrations differs from the SHA-256
# recorded in scripts/migrations.sha256, a recorded one is missing, one is not recorded yet, or versions (the NNNN_
# prefix) are not unique and increasing in the order recorded.
#
# Usage:
#   scripts/check-migrations.sh              check
#   scripts/check-migrations.sh --add-new    record every migration not listed yet (never changes an existing entry)
#   scripts/check-migrations.sh --self-test  prove the check fails on an edited, removed or unrecorded migration
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LIST=scripts/migrations.sha256
DIR=crates/db/migrations

# version FILE: the numeric prefix of NNNN_name.sql, or nothing.
version() {
  [[ "${1##*/}" =~ ^([0-9]+)_ ]] && echo "$((10#${BASH_REMATCH[1]}))"
}

# check ROOT: prints one line per problem to stderr; returns 1 if there is one.
check() {
  local root="$1" status=0 sum file rel number last=-1
  while read -r sum file; do
    [[ -n "$sum" ]] || continue
    number="$(version "$file")" || true
    if [[ -z "$number" ]]; then
      echo "check-migrations: $file has no version prefix (NNNN_name.sql)" >&2
      status=1
    elif ((number <= last)); then
      echo "check-migrations: $file: version $number is not above the previous migration's; versions are unique and increasing" >&2
      status=1
    else
      last="$number"
    fi
    if [[ ! -f "$root/$file" ]]; then
      echo "check-migrations: $file is recorded but missing; never remove an applied migration" >&2
      status=1
    elif [[ "$(sha256sum < "$root/$file" | cut -d ' ' -f 1)" != "$sum" ]]; then
      echo "check-migrations: $file differs from its recorded checksum; never edit a migration, add a new numbered file instead" >&2
      status=1
    fi
  done < "$root/$LIST"
  for file in "$root/$DIR"/*.sql; do
    [[ -e "$file" ]] || continue
    rel="${file#"$root/"}"
    if ! cut -d ' ' -f 3 "$root/$LIST" | grep -qxF -- "$rel"; then
      echo "check-migrations: $rel is not recorded; run scripts/check-migrations.sh --add-new and commit $LIST" >&2
      status=1
      number="$(version "$rel")" || true
      if [[ -z "$number" ]] || ((number <= last)); then
        echo "check-migrations: $rel needs a version prefix above $last (NNNN_name.sql)" >&2
      fi
    fi
  done
  return "$status"
}

# add_new ROOT: appends `sha256  path` for every unrecorded migration.
add_new() {
  local root="$1" file rel
  for file in "$root/$DIR"/*.sql; do
    [[ -e "$file" ]] || continue
    rel="${file#"$root/"}"
    if ! cut -d ' ' -f 3 "$root/$LIST" | grep -qxF -- "$rel"; then
      printf '%s  %s\n' "$(sha256sum < "$file" | cut -d ' ' -f 1)" "$rel" >> "$root/$LIST"
      echo "check-migrations: recorded $rel"
    fi
  done
}

self_test() {
  local failures=0
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  fresh() {
    rm -rf "${tmp:?}/root"
    mkdir -p "$tmp/root/scripts" "$tmp/root/$DIR"
    cp "$ROOT/$LIST" "$tmp/root/$LIST"
    cp "$ROOT/$DIR"/*.sql "$tmp/root/$DIR/"
  }
  expect() { # expect NAME pass|fail
    local got=pass
    check "$tmp/root" 2> /dev/null || got=fail
    if [[ "$got" == "$2" ]]; then echo "ok   $1"; else echo "FAIL $1: expected $2, got $got"; failures=$((failures + 1)); fi
  }
  fresh; expect "recorded migrations are unchanged" pass
  fresh; echo "-- edited" >> "$tmp/root/$DIR/0001_init.sql"; expect "edited migration" fail
  add_new "$tmp/root" > /dev/null; expect "--add-new does not accept an edited migration" fail
  fresh; rm "$tmp/root/$DIR/0001_init.sql"; expect "removed migration" fail
  fresh; echo "SELECT 1;" > "$tmp/root/$DIR/9999_new.sql"; expect "unrecorded new migration" fail
  add_new "$tmp/root" > /dev/null; expect "new migration recorded with --add-new" pass
  fresh; echo "SELECT 1;" > "$tmp/root/$DIR/0002_a.sql"; echo "SELECT 2;" > "$tmp/root/$DIR/0002_b.sql"
  add_new "$tmp/root" > /dev/null; expect "two migrations with the same version" fail
  fresh; echo "SELECT 1;" > "$tmp/root/$DIR/0000_early.sql"
  add_new "$tmp/root" > /dev/null; expect "new migration numbered below a recorded one" fail
  fresh; echo "SELECT 1;" > "$tmp/root/$DIR/later.sql"
  add_new "$tmp/root" > /dev/null; expect "migration without a version prefix" fail
  if ((failures)); then echo "self-test failed: $failures case(s)" >&2; return 1; fi
  echo "check-migrations: self-test passed"
}

case "${1:-}" in
  "") check "$ROOT" && echo "check-migrations: every recorded migration is unchanged" ;;
  --add-new) add_new "$ROOT" ;;
  --self-test) self_test ;;
  *) echo "usage: $0 [--add-new | --self-test]" >&2; exit 2 ;;
esac
