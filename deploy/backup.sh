#!/usr/bin/env bash
# Online backup of a diffpilot deployment (docs/runbook.md, "Backup and restore"); the services keep running.
# Parts: the SQLite database as a consistent snapshot (`VACUUM INTO`; the server keeps it in WAL mode, so readers
# never block its writes), the deployment's .env and secrets directory. Runtime caches are excluded.
# Gateway state, worker volumes and assessment snapshot files need the separate stopped backup in the runbook.
# Prints names and sizes only; encrypt backups before moving them off the host.
#
# Usage (root):
#   deploy/backup.sh                 back up
#   deploy/backup.sh --restore DIR   restore the database of DIR (server and runner stopped);
#                                    the replaced ones are kept in pre-restore-<UTC>/ of each volume
#   deploy/backup.sh --self-test     back up and restore a throwaway database under a temp dir
# Env: SR_BACKUP_DIR (/srv/diffpilot/backup), SR_BACKUP_KEEP (14), SR_COMPOSE_PROJECT (diffpilot),
#      SR_ENV_FILE (/srv/diffpilot/.env), SR_SECRETS_DIR (/srv/diffpilot/secrets); tests only:
#      SR_BACKUP_DATA_DIR (directory instead of the server-data volume), SR_BACKUP_STAMP.
# Needs sqlite3 3.27 or newer on the host (`apt-get install sqlite3`).
set -euo pipefail

project="${SR_COMPOSE_PROJECT:-diffpilot}"
backup_root="${SR_BACKUP_DIR:-/srv/diffpilot/backup}"
keep="${SR_BACKUP_KEEP:-14}"
env_file="${SR_ENV_FILE:-/srv/diffpilot/.env}"
secrets_dir="${SR_SECRETS_DIR:-/srv/diffpilot/secrets}"
db_name=diffpilot.sqlite

say() { echo "backup: $*"; }
die() { echo "backup: $*" >&2; exit 1; }

# volume_dir NAME: the host directory of a compose volume, or the test override.
volume_dir() {
  local override="$1" volume="$2" dir
  if [[ -n "$override" ]]; then echo "$override"; return; fi
  dir="$(docker volume inspect -f '{{.Mountpoint}}' "${project}_$volume" 2> /dev/null)" ||
    die "volume ${project}_$volume not found; check SR_COMPOSE_PROJECT and that this runs as root"
  echo "$dir"
}

# back_up: writes one dated backup directory (complete = renamed from .partial-<stamp> after its SHA256SUMS
# verified) and prunes old complete ones and leftover partials.
back_up() {
  local data stamp out partial
  command -v sqlite3 > /dev/null || die "sqlite3 is required (apt-get install sqlite3)"
  data="$(volume_dir "${SR_BACKUP_DATA_DIR:-}" server-data)"
  [[ -f "$data/$db_name" ]] || die "no database at $data/$db_name"
  stamp="${SR_BACKUP_STAMP:-$(date -u +%Y%m%dT%H%M%SZ)}"
  umask 077
  install -d -m 0700 "$backup_root"
  out="$backup_root/$stamp"
  partial="$backup_root/.partial-$stamp"
  [[ ! -e "$out" ]] || die "$out exists already"
  rm -rf -- "$partial"
  mkdir -m 0700 "$partial"

  # A consistent snapshot in one read transaction; the escaped path is a SQL string literal.
  sqlite3 "$data/$db_name" "VACUUM INTO '${partial//\'/\'\'}/$db_name'" || die "the database snapshot failed"
  [[ "$(sqlite3 "$partial/$db_name" 'PRAGMA integrity_check')" == ok ]] ||
    die "the database snapshot fails its integrity check"
  say "database snapshot $(stat -c %s "$partial/$db_name") bytes"

  if [[ -f "$env_file" ]]; then
    cp -- "$env_file" "$partial/env" || die "cannot copy the env file"
    say "env file copied"
  fi
  if [[ -d "$secrets_dir" ]]; then
    tar -C "$secrets_dir" --numeric-owner -czf "$partial/secrets.tar.gz" . || die "the secrets archive failed"
    say "secrets directory archived"
  fi
  (cd "$partial" && sha256sum -- * > SHA256SUMS && sha256sum --quiet -c SHA256SUMS) ||
    die "the checksums of $partial could not be written or verified"
  mv -- "$partial" "$out"

  # Only complete backups count; the one just written is the newest, so it always stays (keep >= 1).
  find "$backup_root" -mindepth 1 -maxdepth 1 -type d -name '.partial-*' -print0 |
    while IFS= read -r -d '' old; do rm -rf -- "$old" && say "removed the unfinished backup $(basename "$old")"; done
  if [[ "$keep" =~ ^[1-9][0-9]*$ ]]; then
    find "$backup_root" -mindepth 2 -maxdepth 2 -name SHA256SUMS -path "$backup_root/[0-9]*T[0-9]*Z/*" -printf '%h\n' |
      sort | head -n "-$keep" |
      while read -r old; do rm -rf -- "$old" && say "removed the old backup $(basename "$old")"; done
  fi
  say "done: $out"
}

# running_services: compose services of the project that are running now (a read-only query).
running_services() {
  docker ps --filter "label=com.docker.compose.project=$project" --format '{{.Label "com.docker.compose.service"}}'
}

# restore DIR: database back into its volume; the replaced ones go to pre-restore-<UTC>/
# in the same volume. Refuses while the server or the runner runs.
restore() {
  local dir="$1" data owner moved running when
  [[ -f "$dir/SHA256SUMS" && -f "$dir/$db_name" ]] || die "$dir is not a backup directory of this script"
  (cd "$dir" && sha256sum --quiet -c SHA256SUMS) || die "checksums of $dir do not match; the backup is damaged"
  running="$(running_services)" || die "cannot ask Docker which services run; is this root?"
  if grep -qxE 'server|runner' <<< "$running"; then die "stop the server and the runner first (sr stop server runner)"; fi
  [[ "$(sqlite3 "$dir/$db_name" 'PRAGMA integrity_check')" == ok ]] || die "the backed-up database fails its integrity check"
  data="$(volume_dir "${SR_BACKUP_DATA_DIR:-}" server-data)"
  when="$(date -u +%Y%m%dT%H%M%SZ)"

  # Prepare everything next to its target first, so a failure here changes nothing.
  owner="$(stat -c %u:%g "$data")"
  install -m 0600 -o "${owner%:*}" -g "${owner#*:}" "$dir/$db_name" "$data/$db_name.restoring"
  moved="$data/pre-restore-$when"
  mkdir -m 0700 "$moved"
  for f in "$db_name" "$db_name-wal" "$db_name-shm"; do [[ ! -e "$data/$f" ]] || mv -- "$data/$f" "$moved/"; done
  mv -- "$data/$db_name.restoring" "$data/$db_name"
  say "database restored; the replaced one is in $moved"
  say "restored from $dir; delete the pre-restore-$when directories once the service works. The env file and"
  say "secrets stay as they are (copy them by hand if needed)"
}

self_test() {
  local db rows writer
  tmp="$(mktemp -d)"
  trap '[[ -z "${writer:-}" ]] || kill "$writer" 2> /dev/null; rm -rf "$tmp"' EXIT
  fail() { echo "backup self-test: $*" >&2; exit 1; }
  export SR_BACKUP_DATA_DIR="$tmp/data"
  backup_root="$tmp/backup" env_file="$tmp/.env" secrets_dir="$tmp/secrets" keep=2
  mkdir -p "$tmp/data" "$tmp/secrets"
  echo "SR_SECRET_KEY=CANARY-key" > "$env_file"
  echo CANARY-token > "$tmp/secrets/sr-memcan-token"
  db="$tmp/data/$db_name"
  sqlite3 "$db" 'PRAGMA journal_mode=WAL; CREATE TABLE t (v INTEGER); INSERT INTO t VALUES (1);' > /dev/null
  # A writer with committed rows still only in the WAL holds the write lock with an uncommitted row while the
  # backup runs (as the live server may).
  python3 -I -c 'import sqlite3, sys, time
c = sqlite3.connect(sys.argv[1], isolation_level=None)
c.execute("PRAGMA wal_autocheckpoint=0"); c.execute("INSERT INTO t VALUES (2), (3)")
c.execute("BEGIN IMMEDIATE"); c.execute("INSERT INTO t VALUES (99)")
open(sys.argv[2], "w").close(); time.sleep(30)' "$db" "$tmp/locked" &
  writer=$!
  for _ in $(seq 100); do [[ -e "$tmp/locked" ]] && break; sleep 0.1; done
  [[ -e "$tmp/locked" ]] || fail "the writer did not start"

  out_log="$(SR_BACKUP_STAMP=20260101T000001Z back_up 2>&1)" || fail "backup with a live writer failed: $out_log"
  local b="$tmp/backup/20260101T000001Z"
  ! grep -q CANARY <<< "$out_log" || fail "the backup printed a secret"
  [[ "$(stat -c %a "$b")" == 700 ]] || fail "backup directory is not mode 0700"
  for f in "$b"/*; do [[ "$(stat -c %a "$f")" == 600 ]] || fail "$(basename "$f") is not mode 0600"; done
  rows="$(sqlite3 "$b/$db_name" 'SELECT group_concat(v) FROM t')"
  [[ "$rows" == 1,2,3 ]] || fail "snapshot holds '$rows', expected the committed rows 1,2,3"
  [[ ! -e "$b/runner-state.tar.gz" ]] || fail "legacy runner state was archived"
  [[ -f "$b/env" && -f "$b/secrets.tar.gz" ]] || fail "env file or secrets missing"
  kill "$writer"; wait "$writer" 2> /dev/null || true; writer=""

  # Restore after a crashed writer left committed data in the WAL; retain the replaced database.
  python3 -I -c 'import os, sqlite3, sys
c = sqlite3.connect(sys.argv[1], isolation_level=None)
c.execute("PRAGMA wal_autocheckpoint=0"); c.execute("INSERT INTO t VALUES (4)")
os._exit(0)' "$db"
  [[ -s "$db-wal" ]] || fail "the test could not leave a WAL behind"
  # Refused while the stack runs (stubbed query); nothing changes.
  running_services() { echo runner; }
  if (restore "$b" > /dev/null 2>&1); then fail "restore ran while the runner was running"; fi
  compgen -G "$tmp/data/pre-restore-*" > /dev/null && fail "a refused restore moved the current state"
  running_services() { :; }
  out_log="$(restore "$b" 2>&1)" || fail "restore failed: $out_log"
  ! grep -q CANARY <<< "$out_log" || fail "the restore printed a secret"
  [[ "$(sqlite3 "$db" 'SELECT group_concat(v) FROM t')" == 1,2,3 ]] || fail "restored database differs"
  local pre_data
  pre_data="$(compgen -G "$tmp/data/pre-restore-*")" || fail "no pre-restore copy of the database"
  [[ "$(sqlite3 "$pre_data/$db_name" 'SELECT group_concat(v) FROM t')" == 1,2,3,4 ]] ||
    fail "the pre-restore database lost its WAL rows"
  echo corrupt >> "$b/$db_name"
  if (restore "$b" > /dev/null 2>&1); then fail "a damaged backup was restored"; fi

  # Retention: SR_BACKUP_KEEP=2 keeps the two newest complete backups; failed runs leave only .partial-* that
  # neither count nor push out complete ones, and the next run removes them.
  SR_BACKUP_STAMP=20260101T000002Z back_up > /dev/null
  SR_BACKUP_STAMP=20260101T000003Z back_up > /dev/null
  complete() { find "$tmp/backup" -mindepth 1 -maxdepth 1 -type d | sort | xargs -n1 basename | tr '\n' ' '; }
  [[ "$(complete)" == "20260101T000002Z 20260101T000003Z " ]] ||
    fail "retention did not keep exactly the two newest: $(complete)"
  mkdir -p "$tmp/bad" && echo "not a database" > "$tmp/bad/$db_name"
  for n in 4 5 6; do
    if (SR_BACKUP_DATA_DIR="$tmp/bad" SR_BACKUP_STAMP="20260101T00000${n}Z" back_up > /dev/null 2>&1); then
      fail "a backup of a broken database succeeded"
    fi
  done
  [[ "$(complete)" == ".partial-20260101T000004Z .partial-20260101T000005Z .partial-20260101T000006Z 20260101T000002Z 20260101T000003Z " ]] ||
    fail "failed runs changed the complete backups: $(complete)"
  SR_BACKUP_STAMP=20260101T000007Z back_up > /dev/null
  [[ "$(complete)" == "20260101T000003Z 20260101T000007Z " ]] ||
    fail "after failed runs, retention or partial clean-up is wrong: $(complete)"
  [[ -f "$tmp/backup/20260101T000007Z/SHA256SUMS" ]] || fail "complete backup without SHA256SUMS"
  echo "backup: self-test passed"
}

case "${1:-}" in
  "") back_up ;;
  --restore) restore "${2:?--restore needs a backup directory}" ;;
  --self-test) self_test ;;
  *) echo "usage: $0 [--restore DIR | --self-test]" >&2; exit 2 ;;
esac
