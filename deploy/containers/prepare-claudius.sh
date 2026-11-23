#!/bin/sh
# Produce a curated immutable build context from the approved, read-only checkout.
set -eu
source_tree=${1:?reference checkout required}
context=${2:?empty output directory required}
pin=71096d7d4481977added8761b2cfd63cd84852b4
[ "$(git -C "$source_tree" rev-parse HEAD)" = "$pin" ]
mkdir "$context"
git -C "$source_tree" archive "$pin" \
    LICENSE skills/check-pr-comments skills/report-format skills/severity \
    skills/coding-best-practices/SKILL.md skills/review-changes/references/execution-paths.md \
    skills/git-and-github/references/pr-review.md scripts/validate_report.py scripts/severity_util.py \
    schemas/review-report.schema.json tests/fixtures/reports/v4-comment-assessments-host.json \
    | tar -x -C "$context"
printf '%s\n' "$pin" > "$context/SOURCE_COMMIT"
printf '%s\n' "https://github.com/lklimek/claudius/tree/$pin (GPL-3.0)" > "$context/SOURCE"
