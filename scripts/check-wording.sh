#!/usr/bin/env bash
# Enforces the wording rules of docs/design-brief.md §1 (trust rules) on web UI text,
# docs/implementation-plan.md §12.4 (rule A9).
#
# Usage:
#   scripts/check-wording.sh                  check web/src
#   scripts/check-wording.sh --src DIR        check another source directory
#   scripts/check-wording.sh --self-test      prove the check fails on injected violations
#
# Checks string literal values, template literal static parts and JSX text (scripts/check-wording.mjs,
# TypeScript 6 parser) in .ts/.tsx/.js/.jsx/.mjs/.cjs/.mts/.cts/.json files; property names, type
# positions, module paths, test files (*.test.*, *.spec.*, __tests__/), the directories in `exclude` of
# scripts/wording-rules.json (My work, ADR-016) and prompts/ (which must name the
# banned words) are not checked. Files that do not parse fail the check (exit 2).
# `// wording-check: allow <reason>` (or a block comment) exempts percentages on its line, never words.
# Needs node and TypeScript 6: CHECK_WORDING_TYPESCRIPT=<package dir>, else web/node_modules/@typescript/typescript6
# or web/node_modules/typescript. Fails when they cannot be loaded.
# Not caught here (left to the rendered-text Playwright tests, plan §12.4): text assembled at runtime
# from fragments, text outside web/src, decimals.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TYPESCRIPT="${CHECK_WORDING_TYPESCRIPT:-}"
if [[ -z "$TYPESCRIPT" ]]; then
  TYPESCRIPT="$ROOT/web/node_modules/typescript"
  [[ -d "$ROOT/web/node_modules/@typescript/typescript6" ]] && TYPESCRIPT="$ROOT/web/node_modules/@typescript/typescript6"
fi

# Prints one line per violation (`file:line: "match"`); prints nothing when the sources are clean.
violations() {
  local dir="${1%/}"
  find "$dir" -name __tests__ -prune -o -type f \
    \( -name '*.ts' -o -name '*.tsx' -o -name '*.js' -o -name '*.jsx' -o -name '*.mjs' -o -name '*.cjs' \
       -o -name '*.mts' -o -name '*.cts' -o -name '*.json' \) \
    ! -name '*.test.*' ! -name '*.spec.*' -print0 \
    | sort -z | node "$ROOT/scripts/check-wording.mjs" "$TYPESCRIPT" "$dir"
}

# Exit 0 when clean; otherwise prints the violations to stderr and exits 1.
check() {
  local found
  if [[ ! -d "$1" ]]; then
    echo "wording check: nothing to check ($1 does not exist)"
    return 0
  fi
  found="$(violations "$1")"
  if [[ -n "$found" ]]; then
    printf 'wording check failed (design brief §1 trust rules):\n%s\n' "$found" >&2
    return 1
  fi
  echo "wording check passed"
}

# shellcheck disable=SC2016 # test inputs contain literal JavaScript template strings
self_test() {
  local failures=0 n=0
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT

  # case_dir FILE CONTENT: writes CONTENT to FILE in a fresh source dir "$tmp/$n"
  case_dir() {
    n=$((n + 1))
    mkdir -p "$tmp/$n/$(dirname "$1")"
    printf '%s\n' "$2" > "$tmp/$n/$1"
  }
  # expect_pass NAME FILE CONTENT / expect_fail NAME FILE CONTENT PATTERN
  expect_pass() {
    local out
    case_dir "$2" "$3"
    out="$(violations "$tmp/$n")"
    if [[ -n "$out" ]]; then
      echo "FAIL $1: expected no violation, got: $out"
      failures=$((failures + 1))
    else
      echo "ok   $1"
    fi
  }
  expect_fail() {
    local out
    case_dir "$2" "$3"
    out="$(violations "$tmp/$n")"
    if grep -qF -- "$4" <<< "$out"; then
      echo "ok   $1"
    else
      echo "FAIL $1: expected a violation containing '$4', got: '${out}'"
      failures=$((failures + 1))
    fi
  }

  # Banned wording (brief §1, plan §12.4), in each kind of string and in JSX text.
  expect_fail "no risk"            a.ts  "const s = 'No risk';"                       "a.ts:1: \"No risk\""
  expect_fail "no risks"           a.ts  'const s = "no risks found";'                "a.ts:1: \"no risk\""
  expect_fail "nothing to review"  a.ts  'const s = `Nothing to review`;'             "a.ts:1: \"Nothing to review\""
  expect_fail "no review needed"   a.tsx '<p>No review needed</p>'                    "a.tsx:1: \"No review needed\""
  expect_fail "noise"              a.ts  "const o = { label: 'noise' };"              "a.ts:1: \"noise\""
  expect_fail "junk"               a.tsx '<button>Mark as Junk</button>'             "a.tsx:1: \"Junk\""
  expect_fail "verified"           a.tsx '<span title="Verified">x</span>'           "a.tsx:1: \"Verified\""
  expect_fail "was wrong"          a.tsx '<button>Was wrong</button>'                 "a.tsx:1: \"Was wrong\""
  expect_fail "confidence"         a.ts  "const s = 'High confidence';"               "a.ts:1: \"confidence\""
  expect_fail "percentage"         a.ts  "const s = '42% covered';"                   "a.ts:1: \"42%\""
  expect_fail "spaced percentage"  a.ts  "const s = '42 %';"                          "a.ts:1: \"42 %\""
  expect_fail "computed percentage" a.tsx '<b>{`${share}%`}</b>'                      "a.tsx:1: \"{}%\""
  expect_fail "JSX computed percentage" a.tsx '<p>{share}% covered</p>'               "a.tsx:1: \"{}%\""
  expect_fail "multi-line JSX text" a.tsx $'<p>\n  Nothing\n  to review\n</p>'       "a.tsx:2: \"Nothing to review\""
  expect_fail "nested JSX text"    a.tsx '<p>No <b>risk</b> here</p>'                 "a.tsx:1: \"No risk\""
  expect_fail "line number"        a.ts  $'const a = 1;\n\nconst s = "noise";'        "a.ts:3:"
  expect_fail "nested directory"   components/x/a.tsx '<p>No risk</p>'              "components/x/a.tsx:1:"
  expect_fail "json strings"       mocks/a.json '{"label": "verified"}'             "mocks/a.json:1:"
  expect_fail "string after URL"   a.ts  "const u = 'https://x.test'; const s = 'no risk';" "a.ts:1: \"no risk\""
  expect_fail "comment then code"  a.ts  $'/* note */\nconst s = "noise";'          "a.ts:2: \"noise\""
  expect_fail "URL in JSX text"    a.tsx '<p>https://example.com No risk</p>'         "a.tsx:1: \"No risk\""
  expect_fail "comment-like JSX text" a.tsx '<p>/* No risk */</p>'                    "a.tsx:1: \"No risk\""
  expect_fail "after regex literal" a.ts 'const slash = /\//; const text = "No risk";' "a.ts:1: \"No risk\""
  expect_fail "newline escape"     a.ts  'const text = "Nothing\nto review";'         "a.ts:1: \"Nothing to review\""
  expect_fail "tab escape"         a.ts  'const text = "no\trisk";'                   "a.ts:1: \"no risk\""
  expect_fail "unicode escape"     a.ts  'const text = "No\u0020risk";'               "a.ts:1: \"No risk\""
  expect_fail "JSX entity"         a.tsx '<p>No&nbsp;risk</p>'                        "a.tsx:1: \"No risk\""

  expect_fail "adjacent elements"  a.tsx '<section><h2>Status</h2><p>No risk</p></section>' "a.tsx:1: \"No risk\""
  expect_fail "adjacent badge"     a.tsx '<div><span>Status</span><b>Verified</b></div>' "a.tsx:1: \"Verified\""
  expect_fail "adjacent link"      a.tsx '<p><span>Nothing to review</span><a>Details</a></p>' "a.tsx:1: \"Nothing to review\""
  expect_fail "boundary after element" a.tsx '<div><h2>No</h2>risk</div>'               "a.tsx:1: \"No risk\""
  expect_fail "phrase split by element" a.tsx '<p><b>No</b> risk</p>'                "a.tsx:1: \"No risk\""
  expect_fail "JSX text after literal" a.tsx '<p>{"Status"}No risk</p>'              "a.tsx:1: \"No risk\""
  expect_fail "named entity"       a.tsx '<p>No&ensp;risk</p>'                        "a.tsx:1: \"No risk\""
  expect_fail "named entity in attribute" a.tsx '<p title="No&ensp;risk">x</p>'      "a.tsx:1: \"No risk\""
  expect_fail "property value"     a.ts  'const o = { label: "verified" };'          "a.ts:1: \"verified\""
  # The UI must not claim what only the agent's instructions say (brief §1); agent output in .json may.
  expect_fail "execution claim"    a.tsx '<span title="Read the code; nothing was executed.">x</span>' "a.tsx:1: \"nothing was executed\""
  expect_fail "short execution claim" a.ts "const s = 'Nothing executed';"          "a.ts:1: \"Nothing executed\""
  expect_pass "agent output data"  mocks/a.json '{"answer": "Nothing was executed."}'
  # My work is outside the review's trust rules (ADR-016, v1.1 plan §F.5): its components and screen are excluded.
  expect_pass "dashboard component" components/dashboard/x/a.tsx '<p>42% of your PRs</p>'
  expect_pass "dashboard screen"   screens/dashboard/a.ts "const s = '42% done';"
  expect_fail "near the dashboard" components/dashboards/a.tsx '<p>42% done</p>'     "components/dashboards/a.tsx:1:"
  expect_fail "shell of the review routes" screens/shell/ShellLayout.tsx '<p>No risk</p>' "screens/shell/ShellLayout.tsx:1:"
  expect_pass "told not to run"    a.tsx '<span title="The agent was told not to run code.">x</span>'

  # Allow marker: only in a comment, only with a reason, only for the percentage rule.
  expect_pass "allow marker"        a.tsx "<p><div style={{ width: '50%' }} /> {/* wording-check: allow CSS width */}</p>"
  expect_pass "allow line comment"  a.ts  "const w = '50%'; // wording-check: allow CSS width"
  expect_fail "allow marker keeps words" a.tsx '<p><div style={{ width: "50%" }}>No risk</div> {/* wording-check: allow CSS width */}</p>' "a.tsx:1: \"No risk\""
  expect_fail "allow marker in string" a.ts "const s = '50% // wording-check: allow CSS width';" "a.ts:1: \"50%\""
  expect_fail "allow marker in regex" a.ts 'const r = /[// wording-check: allow regex]/; const text = "95% certain";' "a.ts:1: \"95%\""
  expect_fail "allow marker in JSX text" a.tsx '<p>50% // wording-check: allow text</p>' "a.tsx:1: \"50%\""
  expect_fail "allow marker as JSX text" a.tsx '<p>// wording-check: allow text, 50%</p>' "a.tsx:1: \"50%\""
  expect_fail "allow marker without reason" a.tsx "<p><div style={{ width: '50%' }} /> {/* wording-check: allow */}</p>" "a.tsx:1: \"50%\""

  # Not violations: comments, code (identifiers, operators, module paths), tests, other files.
  expect_pass "clean text"          a.tsx '<p>CI check runs: not checked.</p>'
  expect_pass "not analysed"        a.ts  "const s = 'Not analysed';"
  expect_pass "line comment"        a.ts  '// never say "no risk" or "noise"'
  expect_pass "block comment"       a.ts  $'/*\n * verified, confidence, 50%\n */'
  expect_pass "JSX comment"         a.tsx '<div>{/* no review needed */}</div>'
  expect_pass "word parts"          a.ts  "const isVerified = x.verified_by || unverified || noisy;"
  expect_pass "identifier"          a.ts  "const verified = signatureCheck(confidence, noise);"
  expect_pass "modulo"              a.ts  "const odd = i % 2;"
  expect_pass "modulo of literals"  a.ts  "const odd = 42%2;"
  expect_pass "module path"         a.ts  "import { Badge } from './verified-badge';"
  expect_pass "property name"       a.ts  'const o = { "verified": true, noise: 1 };'
  expect_pass "interface member"    a.ts  'interface A { "confidence": string }'
  expect_pass "element access key"  a.ts  'const v = o["verified"];'
  expect_pass "literal type"        a.ts  'type Status = "verified" | `noise`;'
  expect_pass "i18n key"            i18n/en.json '{"verified": "Checked text"}'
  expect_pass "test file"           a.test.tsx "expect(text).not.toContain('no risk');"
  expect_pass "spec file"           a.spec.ts  "expect(text).not.toContain('noise');"
  expect_pass "tests directory"     __tests__/a.ts "const banned = ['verified'];"
  expect_pass "other file types"    a.css '.bar { width: 50%; }'

  # A file that does not parse cannot be checked: exit 2 with the diagnostics.
  expect_error() {
    local code=0
    case_dir "$2" "$3"
    "$0" --src "$tmp/$n" > /dev/null 2>&1 || code=$?
    if ((code == 2)); then
      echo "ok   $1"
    else
      echo "FAIL $1: expected exit 2, got $code"
      failures=$((failures + 1))
    fi
  }
  expect_error "unterminated comment" a.ts $'/* unterminated\nconst s = "No risk";'
  expect_error "broken JSON"          a.json '{"label": '

  # The parser must load; a TypeScript without the classic compiler API (7+) is an error.
  mkdir -p "$tmp/ts7" "$tmp/clean"
  printf "const s = 'fine';\\n" > "$tmp/clean/a.ts"
  printf '{"name": "typescript", "main": "index.js"}\n' > "$tmp/ts7/package.json"
  printf 'module.exports = { version: "7.0.0" };\n' > "$tmp/ts7/index.js"
  for ts in "$tmp/missing-typescript" "$tmp/ts7"; do
    if CHECK_WORDING_TYPESCRIPT="$ts" "$0" --src "$tmp/clean" > /dev/null 2>&1; then
      echo "FAIL unusable TypeScript $ts: expected a non-zero exit"
      failures=$((failures + 1))
    else
      echo "ok   unusable TypeScript $(basename "$ts") fails"
    fi
  done

  # The entry point: missing directory passes, a violation exits non-zero.
  if "$0" --src "$tmp/missing" > /dev/null 2>&1; then
    echo "ok   missing directory passes"
  else
    echo "FAIL missing directory: expected exit 0"
    failures=$((failures + 1))
  fi
  if "$0" --src "$tmp/1" > /dev/null 2>&1; then
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
  "") check "$ROOT/web/src" ;;
  --src) check "${2:?--src needs a directory}" ;;
  --self-test) self_test ;;
  *) echo "usage: $0 [--src DIR | --self-test]" >&2; exit 2 ;;
esac
