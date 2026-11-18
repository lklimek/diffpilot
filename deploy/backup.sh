#!/usr/bin/env bash
# Online backup of a smart-review deployment (docs/runbook.md, "Backup and restore"); the services keep running.
# Parts: the SQLite database as a consistent snapshot (`VACUUM INTO`; the server keeps it in WAL mode, so readers
# never block its writes), the runner's Codex credential and session archives (runner-state `credentials/`,
# `sessions/`), the deployment's .env (SR_SECRET_KEY decrypts stored tokens) and secrets directory.
# Output: SR_BACKUP_DIR/<UTC stamp>/ (mode 0700, files 0600) with SHA256SUMS; the newest SR_BACKUP_KEEP stay.
# Prints names and sizes only, never contents. The backup holds credentials: encrypt it before it leaves the host.
#
# Usage (root):
#   deploy/backup.sh                 back up
#   deploy/backup.sh --restore DIR   restore the database and runner-state parts of DIR (server and runner stopped)
#   deploy/backup.sh --self-test     back up and restore a throwaway database and state under a temp dir
# Env: SR_BACKUP_DIR (/srv/smart-review/backup), SR_BACKUP_KEEP (14), SR_COMPOSE_PROJECT (smart-review),
#      SR_ENV_FILE (/srv/smart-review/.env), SR_SECRETS_DIR (/srv/smart-review/secrets); tests only:
#      SR_BACKUP_DATA_DIR, SR_BACKUP_RUNNER_STATE (directories instead of the compose volumes), SR_BACKUP_STAMP.
# Needs sqlite3 3.27 or newer on the host (`apt-get install sqlite3`).
set -euo pipefail

project="${SR_COMPOSE_PROJECT:-smart-review}"
backup_root="${SR_BACKUP_DIR:-/srv/smart-review/backup}"
keep="${SR_BACKUP_KEEP:-14}"
env_file="${SR_ENV_FILE:-/srv/smart-review/.env}"
secrets_dir="${SR_SECRETS_DIR:-/srv/smart-review/secrets}"
db_name=smart-review.sqlite
state_parts=(credentials sessions)

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

# back_up: writes one dated backup directory and prunes old ones.
back_up() {
  local data state stamp out rc
  command -v sqlite3 > /dev/null || die "sqlite3 is required (apt-get install sqlite3)"
  data="$(volume_dir "${SR_BACKUP_DATA_DIR:-}" server-data)"
  state="$(volume_dir "${SR_BACKUP_RUNNER_STATE:-}" runner-state)"
  [[ -f "$data/$db_name" ]] || die "no database at $data/$db_name"
  stamp="${SR_BACKUP_STAMP:-$(date -u +%Y%m%dT%H%M%SZ)}"
  umask 077
  install -d -m 0700 "$backup_root"
  out="$backup_root/$stamp"
  mkdir -m 0700 "$out" || die "$out exists already"

  # A consistent snapshot in one read transaction; the escaped path is a SQL string literal.
  sqlite3 "$data/$db_name" "VACUUM INTO '${out//\'/\'\'}/$db_name'" || die "the database snapshot failed"
  [[ "$(sqlite3 "$out/$db_name" 'PRAGMA integrity_check')" == ok ]] || die "the database snapshot fails its integrity check"
  say "database snapshot $(stat -c %s "$out/$db_name") bytes"

  local parts=()
  for part in "${state_parts[@]}"; do [[ -e "$state/$part" ]] && parts+=("$part"); done
  if ((${#parts[@]})); then
    # The runner may write a session meanwhile: a file changed during the read is exit 1, kept as a warning.
    rc=0
    tar -C "$state" --numeric-owner --warning=no-file-changed -czf "$out/runner-state.tar.gz" "${parts[@]}" || rc=$?
    ((rc <= 1)) || die "the runner-state archive failed"
    say "runner-state (${parts[*]}) $(stat -c %s "$out/runner-state.tar.gz") bytes"
  fi
  if [[ -f "$env_file" ]]; then
    cp -- "$env_file" "$out/env"
    say "env file copied"
  fi
  if [[ -d "$secrets_dir" ]]; then
    tar -C "$secrets_dir" --numeric-owner -czf "$out/secrets.tar.gz" .
    say "secrets directory archived"
  fi
  (cd "$out" && sha256sum -- * > SHA256SUMS)

  if [[ "$keep" =~ ^[1-9][0-9]*$ ]]; then
    find "$backup_root" -mindepth 1 -maxdepth 1 -type d -name '[0-9]*T[0-9]*Z' | sort | head -n "-$keep" |
      while read -r old; do rm -rf -- "$old" && say "removed the old backup $(basename "$old")"; done
  fi
  say "done: $out"
}

# restore DIR: database and runner-state parts back into the volumes; refuses while server or runner runs.
restore() {
  local dir="$1" data state service owner
  [[ -f "$dir/SHA256SUMS" && -f "$dir/$db_name" ]] || die "$dir is not a backup directory of this script"
  (cd "$dir" && sha256sum --quiet -c SHA256SUMS) || die "checksums of $dir do not match; the backup is damaged"
  if [[ -z "${SR_BACKUP_DATA_DIR:-}" ]]; then
    for service in server runner; do
      [[ -z "$(docker ps -q --filter "label=com.docker.compose.project=$project" \
        --filter "label=com.docker.compose.service=$service")" ]] || die "stop the $service first (sr stop)"
    done
  fi
  [[ "$(sqlite3 "$dir/$db_name" 'PRAGMA integrity_check')" == ok ]] || die "the backed-up database fails its integrity check"
  data="$(volume_dir "${SR_BACKUP_DATA_DIR:-}" server-data)"
  state="$(volume_dir "${SR_BACKUP_RUNNER_STATE:-}" runner-state)"
  owner="$(stat -c %u:%g "$data")"
  install -m 0600 -o "${owner%:*}" -g "${owner#*:}" "$dir/$db_name" "$data/$db_name.restoring"
  rm -f -- "$data/$db_name-wal" "$data/$db_name-shm"
  mv -f -- "$data/$db_name.restoring" "$data/$db_name"
  say "database restored"
  if [[ -f "$dir/runner-state.tar.gz" ]]; then
    for part in "${state_parts[@]}"; do rm -rf -- "${state:?}/$part"; done
    tar -C "$state" --numeric-owner -xzpf "$dir/runner-state.tar.gz"
    say "runner-state credentials and sessions restored"
  fi
  say "restored from $dir; env file and secrets stay as they are (copy them by hand if needed)"
}

self_test() {
  local db rows writer
  tmp="$(mktemp -d)"
  trap '[[ -z "${writer:-}" ]] || kill "$writer" 2> /dev/null; rm -rf "$tmp"' EXIT
  fail() { echo "backup self-test: $*" >&2; exit 1; }
  export SR_BACKUP_DATA_DIR="$tmp/data" SR_BACKUP_RUNNER_STATE="$tmp/state"
  backup_root="$tmp/backup" env_file="$tmp/.env" secrets_dir="$tmp/secrets" keep=2
  mkdir -p "$tmp/data" "$tmp/state/credentials/default" "$tmp/state/sessions/7/analytic" "$tmp/state/mirrors/x" \
    "$tmp/secrets"
  echo '{"CANARY":"credential"}' > "$tmp/state/credentials/default/auth.json"
  chmod 0600 "$tmp/state/credentials/default/auth.json"
  echo '{}' > "$tmp/state/sessions/7/analytic/rollout-1.jsonl"
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
  tar -tzf "$b/runner-state.tar.gz" | grep -qx 'credentials/default/auth.json' || fail "credential not archived"
  tar -tzf "$b/runner-state.tar.gz" | grep -q '^sessions/7/analytic/rollout-1.jsonl' || fail "sessions not archived"
  ! tar -tzf "$b/runner-state.tar.gz" | grep -q '^mirrors' || fail "mirrors archived (re-creatable, not wanted)"
  tar -tvzf "$b/runner-state.tar.gz" credentials/default/auth.json | grep -q '^-rw-------' ||
    fail "credential mode not kept"
  [[ -f "$b/env" && -f "$b/secrets.tar.gz" ]] || fail "env file or secrets missing"
  kill "$writer"; wait "$writer" 2> /dev/null || true; writer=""

  # Restore into the (throwaway) volumes after a change that a crashed writer left in the WAL; the change is gone,
  # the backup's state is back.
  python3 -I -c 'import os, sqlite3, sys
c = sqlite3.connect(sys.argv[1], isolation_level=None)
c.execute("PRAGMA wal_autocheckpoint=0"); c.execute("INSERT INTO t VALUES (4)")
os._exit(0)' "$db"
  [[ -s "$db-wal" ]] || fail "the test could not leave a WAL behind"
  rm -rf "$tmp/state/credentials"
  out_log="$(restore "$b" 2>&1)" || fail "restore failed: $out_log"
  [[ "$(sqlite3 "$db" 'SELECT group_concat(v) FROM t')" == 1,2,3 ]] || fail "restored database differs"
  [[ "$(stat -c %a "$tmp/state/credentials/default/auth.json")" == 600 ]] || fail "credential not restored with 0600"
  echo corrupt >> "$b/$db_name"
  if (restore "$b" > /dev/null 2>&1); then fail "a damaged backup was restored"; fi

  # Retention: SR_BACKUP_KEEP=2 keeps the two newest.
  SR_BACKUP_STAMP=20260101T000002Z back_up > /dev/null
  SR_BACKUP_STAMP=20260101T000003Z back_up > /dev/null
  [[ "$(find "$tmp/backup" -mindepth 1 -maxdepth 1 -type d | sort | xargs -n1 basename | tr '\n' ' ')" == \
    "20260101T000002Z 20260101T000003Z " ]] || fail "retention did not keep exactly the two newest: $(ls "$tmp/backup")"
  echo "backup: self-test passed"
}

case "${1:-}" in
  "") back_up ;;
  --restore) restore "${2:?--restore needs a backup directory}" ;;
  --self-test) self_test ;;
  *) echo "usage: $0 [--restore DIR | --self-test]" >&2; exit 2 ;;
esac
