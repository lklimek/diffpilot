#!/usr/bin/env bash
# Enforces the wording rules of docs/design-brief.md §1 (trust rules) on web source text,
# docs/implementation-plan.md §12.4 (rule A9).
#
# Usage:
#   scripts/check-wording.sh                  check web/src
#   scripts/check-wording.sh --src DIR        check another source directory
#   scripts/check-wording.sh --self-test      prove the check fails on injected violations
#
# Scope: web source text only. Prompts must name the banned words, so prompts/ is not checked.
# Comments and test files (*.test.*, *.spec.*, __tests__/) are skipped; everything else in
# .ts/.tsx/.js/.jsx/.mjs/.cjs/.mts/.cts/.json files is checked, so JSX text counts as well as string
# literals. A line containing `wording-check: allow` (with a reason) is exempt.
# Not caught here (left to the rendered-text Playwright tests, plan §12.4): text assembled at runtime
# from fragments, text outside web/src, decimals.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Banned wording: brief §1 "Silence is not safety", "A badge names exactly what was checked",
# "No numeric confidence"; plan §12.4 ("Was wrong", "confidence", percentages incl. `${x}%`).
# Perl program: blanks comments (keeping strings and line breaks), then reports each banned match.
# shellcheck disable=SC2016 # perl program, not shell expansion
SCAN='
  my $dir = shift @ARGV;
  my $banned = qr/\bno\s+risk|\bnothing\s+to\s+review\b|\bno\s+review\s+needed\b|\bnoise\b|\bverified\b
                  |\bwas\s+wrong\b|\bconfidence\b|(?:[0-9]+|\})(?:\x20|\xc2\xa0)?%/xi;
  for my $file (@ARGV) {
    open(my $fh, "<", $file) or die "wording check: cannot read $file: $!\n";
    my $src = do { local $/; <$fh> };
    close $fh;
    my @lines = split /\n/, $src, -1;
    $src =~ s{(\x27(?:\\.|[^\x27\\\n])*\x27|"(?:\\.|[^"\\\n])*"|`(?:\\.|[^`\\])*`)|(//[^\n]*|/\*.*?\*/)}
             {defined $1 ? $1 : ($2 =~ tr/\n//cdr)}gse;
    (my $rel = $file) =~ s{^\Q$dir\E/}{};
    while ($src =~ /$banned/g) {
      my $line = 1 + (substr($src, 0, $-[0]) =~ tr/\n//);
      next if $lines[$line - 1] =~ /wording-check: allow/;
      (my $match = $&) =~ s/\s+/ /g;
      print "$rel:$line: \"$match\"\n";
    }
  }
'

# Prints one line per violation (`file:line: "match"`); prints nothing when the sources are clean.
violations() {
  local dir="${1%/}"
  find "$dir" -name __tests__ -prune -o -type f \
    \( -name '*.ts' -o -name '*.tsx' -o -name '*.js' -o -name '*.jsx' -o -name '*.mjs' -o -name '*.cjs' \
       -o -name '*.mts' -o -name '*.cts' -o -name '*.json' \) \
    ! -name '*.test.*' ! -name '*.spec.*' -print0 \
    | sort -z | xargs -0 -r perl -e "$SCAN" "$dir"
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
  expect_fail "noise"              a.ts  "label: 'noise',"                            "a.ts:1: \"noise\""
  expect_fail "verified"           a.tsx '<span title="Verified">x</span>'           "a.tsx:1: \"Verified\""
  expect_fail "was wrong"          a.tsx '<button>Was wrong</button>'                 "a.tsx:1: \"Was wrong\""
  expect_fail "confidence"         a.ts  "const s = 'High confidence';"               "a.ts:1: \"confidence\""
  expect_fail "percentage"         a.ts  "const s = '42% covered';"                   "a.ts:1: \"42%\""
  expect_fail "spaced percentage"  a.ts  "const s = '42 %';"                          "a.ts:1: \"42 %\""
  expect_fail "computed percentage" a.tsx '<b>{`${share}%`}</b>'                      "a.tsx:1: \"}%\""
  expect_fail "multi-line JSX text" a.tsx $'<p>\n  Nothing\n  to review\n</p>'       "a.tsx:2: \"Nothing"
  expect_fail "line number"        a.ts  $'const a = 1;\n\nconst s = "noise";'        "a.ts:3:"
  expect_fail "nested directory"   components/x/a.tsx '<p>No risk</p>'              "components/x/a.tsx:1:"
  expect_fail "json strings"       mocks/a.json '{"label": "verified"}'             "mocks/a.json:1:"
  expect_fail "string after URL"   a.ts  "const u = 'https://x.test'; const s = 'no risk';" "a.ts:1: \"no risk\""
  expect_fail "comment then code"  a.ts  $'/* note */\nconst s = "noise";'          "a.ts:2: \"noise\""

  # Not violations.
  expect_pass "clean text"          a.tsx '<p>The LLM raised no questions. That is not evidence of no defects.</p>'
  expect_pass "not analysed"        a.ts  "const s = 'Not analysed';"
  expect_pass "line comment"        a.ts  '// never say "no risk" or "noise"'
  expect_pass "block comment"       a.ts  $'/*\n * verified, confidence, 50%\n */'
  expect_pass "JSX comment"         a.tsx '<div>{/* no review needed */}</div>'
  expect_pass "word parts"          a.ts  "const isVerified = x.verified_by || unverified || noisy;"
  expect_pass "modulo"              a.ts  "const odd = i % 2;"
  expect_pass "test file"           a.test.tsx "expect(text).not.toContain('no risk');"
  expect_pass "spec file"           a.spec.ts  "expect(text).not.toContain('noise');"
  expect_pass "tests directory"     __tests__/a.ts "const banned = ['verified'];"
  expect_pass "other file types"    a.css '.bar { width: 50%; }'
  expect_pass "allow marker"        a.tsx "<div style={{ width: '50%' }} /> {/* wording-check: allow CSS width */}"

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
