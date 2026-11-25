#!/usr/bin/env bash
# Enforces the crate dependency rules of docs/implementation-plan.md §3 and §4.1 (A1, A2, A11)
# from `cargo metadata`.
#
# Usage:
#   scripts/check-architecture.sh                    check the workspace
#   scripts/check-architecture.sh --metadata FILE    check a saved `cargo metadata --no-deps` JSON (crate rules only)
#   scripts/check-architecture.sh --self-test        prove the check fails on injected violations
#
# Also from the sources (A11): non-test Rust code in crates/ and tools/ declares no `Result<_, String>`, no
# `Box<dyn ...Error>` and uses no `anyhow::`/`eyre::` paths. Non-test: outside tests/ and benches/, files not named
# *tests.rs, and lines before a file's inline `#[cfg(test)] mod NAME {` (an out-of-line `mod NAME;` is skipped).
#
# Not caught here (left to review, plan §4.2): hand-written GitHub HTTP calls through a generic client
# (reqwest) outside server::github; sqlx types leaking through db's public API (A2); database or GitHub
# crates whose names match none of the patterns below, or that arrive transitively through another
# crate; module-level ownership inside a crate (A3-A6).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Rules. Dependencies of every kind (normal, dev, build) count.
# A1: workspace crates a member may depend on; any other member (tools/*) may depend only on contracts.
#     One exception, dev dependencies only: server's tests run the real client against tools/fake-github.
# A2/§3: crate families only one member may use, matched by name pattern:
#   db only:     sqlx and sqlx-*, diesel*, sea-orm*, sea-query*, SQLite/PostgreSQL/MySQL drivers;
#   server only: GitHub API clients (names containing "octo", "github" or "hubcaps": octocrab, octorust,
#                github-rs, roctogen, ...).
# A11: error crates nobody may use (thiserror typed errors instead).
# shellcheck disable=SC2016 # jq program, not shell expansion
RULES='
  def allowed: {
    contracts: [],
    analysis: ["contracts"],
    db: ["contracts"],
    runner: ["contracts"],
    server: ["contracts", "analysis", "db"]
  };
  def default_allowed: ["contracts"];
  def dev_allowed: { server: ["fake-github"] };
  def owner:
    if test("^(sqlx|diesel|sea-orm|sea-query)([-_].*)?$")
      or test("^(rusqlite|tokio-rusqlite|libsqlite3-sys|postgres|tokio-postgres|mysql|mysql_async)$")
    then "db"
    elif test("octo|github|hubcaps") then "server"
    else null end;
  def forbidden: ["anyhow", "eyre", "color-eyre"];

  [.packages[].name] as $members
  | (allowed | keys[] | select(. as $c | $members | index($c) | not)
      | "A1: expected workspace crate \(.) is missing"),
    (.packages[] as $p
      | $p.dependencies[]
      | (.kind // "normal") as $kind
      | .name as $d
      | (select($members | index($d)) | select((allowed[$p.name] // default_allowed) | index($d) | not)
          | select($kind != "dev" or ((dev_allowed[$p.name] // []) | index($d) | not))
          | "A1: \($p.name) must not depend on workspace crate \($d) (\($kind))"),
        (($d | owner) as $owner | select($owner != null and $owner != $p.name)
          | "A2: \($d) is allowed only in crate \($owner), found in \($p.name) (\($kind))"),
        (select(forbidden | index($d))
          | "A11: \($p.name) depends on \($d) (\($kind)); use thiserror typed errors"))
'

# A11 in the sources (ERE, awk).
ERROR_PATTERN='Result<.*,[[:space:]]*String[[:space:]]*>|Box<dyn [A-Za-z_:]*Error|(^|[^A-Za-z0-9_])(anyhow|eyre)::'
# TODO: temporary allow-list of existing String errors, to be replaced by thiserror enums; remove each entry with
# its fix. Entries: "path|trimmed line".
ERROR_ALLOW=(
  'tools/fake-github/src/app.rs|fn scope(&self, body: &[u8]) -> Result<BTreeMap<String, String>, String> {'
)

# source_violations ROOT: one line per non-allow-listed match in non-test Rust code below ROOT.
source_violations() {
  local root="$1" file rel hit text entry allowed
  while IFS= read -r -d '' file; do
    rel="${file#"$root/"}"
    while IFS= read -r hit; do
      text="${hit#*|}"
      allowed=0
      for entry in "${ERROR_ALLOW[@]}"; do [[ "$entry" == "$rel|$text" ]] && allowed=1; done
      ((allowed)) || echo "A11: $rel:${hit%%|*}: $text; use a thiserror enum"
    done < <(awk -v pat="$ERROR_PATTERN" '
      pending && /^[[:space:]]*$/ { next }
      pending && /^[[:space:]]*(pub(\([a-z]+\))?[[:space:]]+)?mod[[:space:]]+[A-Za-z0-9_]+[[:space:]]*;/ { pending = 0; next }
      pending && /^[[:space:]]*(pub(\([a-z]+\))?[[:space:]]+)?mod[[:space:]]/ { exit }
      { pending = 0 }
      /^[[:space:]]*#\[cfg\(test\)\]/ { pending = 1; next }
      /^[[:space:]]*\/\// { next }
      $0 ~ pat { line = $0; sub(/^[[:space:]]+/, "", line); print FNR "|" line }' "$file")
  done < <(find "$root/crates" "$root/tools" -name '*.rs' -not -path '*/tests/*' -not -path '*/benches/*' \
    -not -name '*tests.rs' -print0 2> /dev/null | sort -z)
}

# source_check ROOT: exit 0 when the sources are clean; otherwise prints the violations to stderr and returns 1.
source_check() {
  local found
  found="$(source_violations "$1")"
  if [[ -n "$found" ]]; then
    printf 'architecture source check failed:\n%s\n' "$found" >&2
    return 1
  fi
  echo "architecture source check passed"
}

# Prints one line per violation of the rules; prints nothing when the metadata is clean.
violations() {
  jq -r "$RULES" "$1"
}

# Exit 0 when clean; otherwise prints the violations to stderr and exits 1.
check() {
  local found
  found="$(violations "$1")"
  if [[ -n "$found" ]]; then
    printf 'architecture check failed:\n%s\n' "$found" >&2
    return 1
  fi
  echo "architecture check passed"
}

workspace_metadata() {
  cargo metadata --format-version 1 --no-deps --manifest-path "$ROOT/Cargo.toml"
}

self_test() {
  local real failures=0
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  real="$tmp/real.json"
  workspace_metadata > "$real"

  # expect_pass NAME FILE / expect_fail NAME FILE PATTERN
  expect_pass() {
    if violations "$2" | grep -q .; then
      echo "FAIL $1: expected no violation, got: $(violations "$2")"
      failures=$((failures + 1))
    else
      echo "ok   $1"
    fi
  }
  expect_fail() {
    local out
    out="$(violations "$2")"
    if grep -qF -- "$3" <<< "$out"; then
      echo "ok   $1"
    else
      echo "FAIL $1: expected a violation containing '$3', got: '${out}'"
      failures=$((failures + 1))
    fi
  }
  # with_dep PACKAGE DEPENDENCY KIND(normal|dev|build) > file
  with_dep() {
    jq --arg p "$1" --arg d "$2" --arg k "$3" \
      '(.packages[] | select(.name == $p) | .dependencies) +=
         [{name: $d, kind: (if $k == "normal" then null else $k end)}]' "$real"
  }
  # with_member NAME DEPENDENCY > file: adds a workspace member (e.g. tools/fake-github)
  with_member() {
    jq --arg n "$1" --arg d "$2" '.packages += [{name: $n, dependencies: [{name: $d, kind: null}]}]' "$real"
  }

  expect_pass "current workspace is clean" "$real"
  with_dep runner db normal > "$tmp/1.json";          expect_fail "runner -> db" "$tmp/1.json" "runner must not depend on workspace crate db"
  with_dep runner analysis normal > "$tmp/2.json";    expect_fail "runner -> analysis" "$tmp/2.json" "runner must not depend on workspace crate analysis"
  with_dep analysis db normal > "$tmp/3.json";        expect_fail "analysis -> db" "$tmp/3.json" "analysis must not depend on workspace crate db"
  with_dep db analysis normal > "$tmp/4.json";        expect_fail "db -> analysis" "$tmp/4.json" "db must not depend on workspace crate analysis"
  with_dep contracts db normal > "$tmp/5.json";       expect_fail "contracts -> db" "$tmp/5.json" "contracts must not depend on workspace crate db"
  with_dep server runner dev > "$tmp/6.json";         expect_fail "server -> runner (dev)" "$tmp/6.json" "server must not depend on workspace crate runner"
  with_dep analysis server build > "$tmp/7.json";     expect_fail "analysis -> server (build)" "$tmp/7.json" "analysis must not depend on workspace crate server"
  with_dep server sqlx normal > "$tmp/8.json";        expect_fail "sqlx outside db" "$tmp/8.json" "sqlx is allowed only in crate db, found in server"
  with_dep runner libsqlite3-sys normal > "$tmp/9.json"; expect_fail "libsqlite3-sys outside db" "$tmp/9.json" "libsqlite3-sys is allowed only in crate db, found in runner"
  with_dep runner octocrab normal > "$tmp/10.json";   expect_fail "octocrab outside server" "$tmp/10.json" "octocrab is allowed only in crate server, found in runner"
  with_dep server sqlx-postgres normal > "$tmp/16.json"; expect_fail "sqlx-* family outside db" "$tmp/16.json" "sqlx-postgres is allowed only in crate db, found in server"
  with_dep analysis diesel normal > "$tmp/17.json";   expect_fail "diesel outside db" "$tmp/17.json" "diesel is allowed only in crate db, found in analysis"
  with_dep contracts sea-orm dev > "$tmp/18.json";    expect_fail "sea-orm outside db" "$tmp/18.json" "sea-orm is allowed only in crate db, found in contracts"
  with_dep runner tokio-postgres normal > "$tmp/19.json"; expect_fail "postgres driver outside db" "$tmp/19.json" "tokio-postgres is allowed only in crate db, found in runner"
  with_dep runner github-rs normal > "$tmp/20.json";  expect_fail "github-rs outside server" "$tmp/20.json" "github-rs is allowed only in crate server, found in runner"
  with_dep runner octorust normal > "$tmp/21.json";   expect_fail "octorust outside server" "$tmp/21.json" "octorust is allowed only in crate server, found in runner"
  with_dep analysis hubcaps normal > "$tmp/22.json";  expect_fail "hubcaps outside server" "$tmp/22.json" "hubcaps is allowed only in crate server, found in analysis"
  with_dep db sqlx-sqlite normal > "$tmp/23.json";    expect_pass "sqlx-sqlite in db" "$tmp/23.json"
  with_dep server octocrab normal > "$tmp/24.json";   expect_pass "octocrab in server" "$tmp/24.json"
  with_dep analysis anyhow normal > "$tmp/11.json";   expect_fail "anyhow" "$tmp/11.json" "analysis depends on anyhow"
  with_dep server eyre dev > "$tmp/12.json";          expect_fail "eyre (dev)" "$tmp/12.json" "server depends on eyre"
  with_member fake-github server > "$tmp/13.json";    expect_fail "tool -> server" "$tmp/13.json" "fake-github must not depend on workspace crate server"
  with_member fake-github contracts > "$tmp/14.json"; expect_pass "tool -> contracts" "$tmp/14.json"
  with_dep server fake-github dev > "$tmp/25.json";   expect_pass "server -> fake-github (dev)" "$tmp/25.json"
  with_dep server fake-github normal > "$tmp/26.json"; expect_fail "server -> fake-github (normal)" "$tmp/26.json" "server must not depend on workspace crate fake-github (normal)"
  with_dep server fake-github build > "$tmp/27.json"; expect_fail "server -> fake-github (build)" "$tmp/27.json" "server must not depend on workspace crate fake-github (build)"
  with_dep runner fake-github dev > "$tmp/28.json";   expect_fail "runner -> fake-github (dev)" "$tmp/28.json" "runner must not depend on workspace crate fake-github (dev)"
  jq 'del(.packages[] | select(.name == "db"))' "$real" > "$tmp/15.json"
  expect_fail "missing crate" "$tmp/15.json" "expected workspace crate db is missing"

  # Source check (A11): a tree with one violation per form, test code and comments that must pass.
  src="$tmp/src"
  mkdir -p "$src/crates/x/src" "$src/crates/x/tests" "$src/tools/t/src"
  expect_source() { # expect_source NAME pass|fail FILE CONTENT
    rm -rf "${src:?}/crates/x/src"/* "$src/crates/x/tests"/* "$src/tools/t/src"/*
    printf '%s\n' "$4" > "$src/$3"
    local got=pass
    [[ -z "$(source_violations "$src")" ]] || got=fail
    if [[ "$got" == "$2" ]]; then echo "ok   $1"; else echo "FAIL $1: expected $2, got $got"; failures=$((failures + 1)); fi
  }
  expect_source "clean source" pass crates/x/src/lib.rs 'fn a() -> Result<(), Error> { Ok(()) }'
  expect_source "Result<_, String>" fail crates/x/src/lib.rs 'fn a() -> Result<(), String> { Ok(()) }'
  expect_source "nested Result<_, String>" fail tools/t/src/main.rs 'fn a() -> Result<Vec<Option<u8>>, String> { todo!() }'
  expect_source "Box<dyn Error>" fail crates/x/src/lib.rs 'fn a() -> Result<(), Box<dyn std::error::Error>> { Ok(()) }'
  expect_source "anyhow path" fail crates/x/src/lib.rs 'fn a() -> anyhow::Result<()> { Ok(()) }'
  expect_source "eyre path" fail crates/x/src/lib.rs 'use eyre::Report;'
  expect_source "a cfg(test) item is not the test module" fail crates/x/src/lib.rs \
    "$(printf '#[cfg(test)]\nuse std::fs;\nfn a() -> Result<(), String> { Ok(()) }')"
  expect_source "out-of-line test module declaration is skipped, not the end" fail crates/x/src/lib.rs \
    "$(printf '#[cfg(test)]\nmod tests;\npub fn a() -> Result<(), String> { Ok(()) }')"
  expect_source "test module" pass crates/x/src/lib.rs \
    "$(printf 'fn a() {}\n#[cfg(test)]\nmod tests {\n    fn b() -> Result<(), String> { Ok(()) }\n}')"
  expect_source "integration test" pass crates/x/tests/it.rs 'fn b() -> Result<(), String> { Ok(()) }'
  expect_source "tests file" pass crates/x/src/concurrency_tests.rs 'fn b() -> Result<(), String> { Ok(()) }'
  expect_source "comment" pass crates/x/src/lib.rs '/// Never returns Result<(), String>.'
  ERROR_ALLOW+=('crates/x/src/lib.rs|fn a() -> Result<(), String> { Ok(()) }')
  expect_source "allow-listed line" pass crates/x/src/lib.rs 'fn a() -> Result<(), String> { Ok(()) }'
  expect_source "allow-list is per line" fail crates/x/src/lib.rs 'fn c() -> Result<(), String> { Ok(()) }'
  expect_pass_source() {
    if source_check "$ROOT" > /dev/null 2>&1; then echo "ok   current sources are clean (allow-list applied)"; else
      echo "FAIL current sources: $(source_violations "$ROOT")"; failures=$((failures + 1)); fi
  }
  expect_pass_source

  # The entry point must exit non-zero on a violation.
  if "$0" --metadata "$tmp/1.json" > /dev/null 2>&1; then
    echo "FAIL check exits 0 on a violation"
    failures=$((failures + 1))
  else
    echo "ok   check exits non-zero on a violation"
  fi

  if ((failures > 0)); then
    echo "self-test failed: $failures case(s)" >&2
    return 1
  fi
  echo "self-test passed"
}

case "${1:-}" in
  "")
    metadata="$(mktemp)"
    trap 'rm -f "$metadata"' EXIT
    workspace_metadata > "$metadata"
    status=0
    check "$metadata" || status=1
    source_check "$ROOT" || status=1
    exit "$status"
    ;;
  --metadata) check "${2:?--metadata needs a file}" ;;
  --self-test) self_test ;;
  *) echo "usage: $0 [--metadata FILE | --self-test]" >&2; exit 2 ;;
esac
