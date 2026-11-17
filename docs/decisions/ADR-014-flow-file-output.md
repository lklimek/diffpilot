# ADR-014: Tagged flow files checked by the agent and server

- Status: accepted (owner, 2026-11-17).
- Supersedes ADR-008's schema-constrained JSON transport for `analyse_flow` only.
- Related: ADR-004 (sessions), ADR-009 (generated contracts), ADR-013 (agent uid).

## Reason

Real runs showed flow wall time dominated by output tokens: writing final JSON
accounted for 58% of flow time. Indentation, repeated keys and repeated paths used
output tokens without adding review information. A file also allows a model to edit
one rejected field instead of emitting a complete replacement document.

## Decision

Flow jobs write `out/flow.md`, following
[the versioned grammar](../contracts/agent-output-format.md). Fixed headings delimit
steps, annotations, assessments and references. Prose/code is raw; multiline values
use a chosen heredoc terminator. Paths are declared once under short ids; planned
hunk references reconstruct file, side, kind and planned span. Ranges combining hunks
or additional context still need explicit locations. The current plan has no symbol
field, so symbols remain optional model output. Empty optional fields are omitted.

`analysis::tagged` contains the parser and invokes the existing full flow verifier.
The `sr-check` binary is a target of `analysis`, installed in `/usr/local/bin` in the
runner image. Both the server and checker call `Context::check`; runner provider
code has no dependency on the analysis library. The crate dependency guard and its
self-test need no rule changes.

`JobSpec.flow_file` carries a versioned, no-secret verification context and optional
rejected contents. The runner writes the context as runner-owned mode 0444, hands
only the empty mode-0700 `out` directory to the agent uid, and copies any repair file
in through the existing helper. Context includes plan, diff, moves, listed thread
aliases and pinned commits. It is not an agent-selected repository path. The agent
can inspect the grammar/catalogue with `sr-check format` / `sr-check catalog`.

The prompt allows writes only to the result file and execution of the installed
checker. Codex does not receive `--output-schema` for these jobs; the final answer
is a one-line completion message. After the agent exits and the supervisor sweeps
its processes, the bounded helper collects a single regular UTF-8 file (maximum
1 MiB, no symlinks, mounts, FIFOs or hard links). Its contents travel as a JSON string
in `JobOutcome::Succeeded.output`. Missing/unsafe/oversized files fail the attempt.
Secrets are scrubbed through the same output path as other provider results.

A checker `ok` is not trusted. The server checks the text again against its own
facts and repository, then stores the existing canonical `FlowAnalysisOutput` JSON.
Verifier rejections use the existing repair budget. Session archives contain model
history, not job files: the server explicitly restores the rejected text in each
repair job and tells the model to edit it. Session routing, token accounting,
stored documents and browser contracts do not change.

Plan, PR context and Ask retain schema-constrained JSON. Migrating their separate
shapes is unnecessary for the measured flow bottleneck; Ask answers are small.

## Verification and limits

The fixture measurement helper is
`analysis::verify::tests::tagged_fixture_checker_matches_server_verifier_and_measures_bytes`.
For five fixture flows it reports 42,070 JSON bytes versus 21,197 tagged bytes
(49.6% fewer), with exact round-trip equality and the full verifier passing. These
are byte counts, not a claim about token savings or real-run latency. Confirm those
with real Codex runs, including an edit-only repair and resumed session.

The checker runs all existing deterministic checks available without secrets,
including pinned-file bounds, coverage, references, assessments and wording. Existing
non-blocking wording warnings stay non-blocking. A model can ignore the checker,
but cannot bypass the server check. The parser accepts no executable directives.

Next steps, explicitly out of scope: stream steps while the file is being written;
a planning cap on steps per flow; lazy flow analysis; other job output formats.
