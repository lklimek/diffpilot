Job: verify_comments. Host brief v1: run check-pr-comments in host-supplied, report-only mode.
Read /work/pr-data/manifest.json and scope.json; assess only their frozen threads and sources.
Filter the supplied files by scope, preserving reply chains; never print whole comment files.
Use /work/repo at its checked-out head, with base conventions from pr-data/conventions/.
Read-only gh searches and git-history reads are allowed; do not refetch supplied PR inputs.
Never build, test or run PR code, change the checkout, post comments or resolve threads.
Write only the 4.2 host-supplied report to /work/out/report.json.
CLAUDE_PLUGIN_ROOT=/etc/codex/claudius; keep sibling skill references under that root.
Validate: python3 /etc/codex/claudius/scripts/validate_report.py /work/out/report.json --ids /work/pr-data/ids.json
Public reply drafts must say nothing about the assessor. Do not read schema or validator source.
