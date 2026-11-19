# Flow file format, version 2

Write `out/flow.md` relative to the job directory (the parent of `repo`). Run
`sr-check flow out/flow.md`, edit the file to fix every reported problem, and finish
with one line: `Flow file ready.` The final message is not the document. Do not
run repository code. Writing this output and running the installed checker are
allowed. The server repeats the check independently.

Read `flow-context.json` for the accepted plan, diff and pinned base/head commits.
The checker derives `p1`, `p2`, ... in diff order, current path followed by a distinct
old path. Each path gets one id. Planned hunk ids are those of the requested flow.
To see the resolved catalogue, use `sr-check catalog out/flow.md`.

## Grammar

Headings and field names are literal, case-sensitive English. Values use the job's
language. Empty optional fields and empty collections are omitted. Unknown fields,
duplicates, unknown ids, invalid enums and misplaced sections are errors. A field
is `key: value` on one line. For raw multiline prose or code use `key: <<END`, the
raw lines, and `END` on a line of its own. Choose any alphanumeric/underscore
terminator absent from the value. Structural lines use LF; raw heredoc bodies preserve
carriage returns and newlines exactly. There is no escaping or Markdown interpretation.
Maximum document size is 1 MiB, with at most 2000 sections (headings). Flow ids, step ids
and thread aliases are 1 to 64 characters of `A-Z`, `a-z`, `0-9`, `_` or `-`; a declared
file id is 1 to 64 letters or digits. A heading word is at most 128 bytes and a field name
at most 80 bytes. Violations are reported with their line.

- `# flow <id>` exactly once. Optional `file <id>: <repo-relative-path>` declarations
  name additional context/evidence files once; never redeclare a known path/id.
- `## introduction`: required `problem_before`, `change`, `example_before_after`.
- `## step <id> <hunk-id>` uses the planned hunk's file, side, span and kind.
  Do not repeat these fields. A head selection uses its planned subrange; a
  deletion-only hunk uses base. A head hunk without removals is `added`, otherwise
  `modified`. When a step connects/combines ranges that are not a single planned
  hunk, use `## step <id> <file-id>:<head|base>:<first>-<last> <kind>`.
  Required: `title`, `decides`, `why`.
  Optional: `called_by`, `next`, `symbol`, `moved_from: <file-id>:base:<first>-<last>`.
  Write `called_by` / `next` only for information absent from the graph (an external
  caller, an external destination or an additional condition). Omitted values are
  derived from incoming/outgoing edges, including step numbers and edge labels;
  a boundary with no such edge stays empty. Explicit prose follows the derived
  targets (`Step 2; <prose>`), so never restate step ids.
- `### annotation <type> <basis> <first>-<last>` under its step. Optional `was`, `is`,
  `text`, `evidence`. Behaviour requires nonempty `was`/`is`; other types require
  `text`. Evidence is space-separated `<file-id>:<side>:<line>` references.
  A `flag` also requires `level` (`red` or `yellow`), `scenario` (2 to 7 steps, one
  per line, unnumbered; list markers are dropped) and `not_checked` (one line: what was
  not reproduced or run); its `text` is the title, starting `Potential ` (Polish:
  `Potencjalny `, `Potencjalna ` or `Potencjalne `) followed by the risk class. Plain
  text with code and types such as `Option<A>` in backticks (no other Markdown, links
  or HTML); title at most 160, scenario steps 10 to 300 and `not_checked` at most 300
  characters; at most 5 flags per step, and a step with a red flag is rated `medium`
  or `high`. Only a flag has these fields.
- `### assessment` under every step, exactly once. Required: `risk`, `reason`. At
  `risk: low` the rest may be omitted (concise assessment: band plus one sentence).
  Above low also required: `headline`, `basis_note`, `not_checked`, `origin`,
  `blocking`. Optional: `smallest_correction`. Flow risk is the highest step risk;
  every step is assessed.
- `## edge <from-step> <to-step>`; optional `label`. Omit all edges for a walk in
  reading order: the parser connects each adjacent pair. If any edge is supplied,
  supply the complete graph, including branches and loops; no implicit edges are added.
- `## impact <step> <file-id>:head:<first>-<last>`; required `reason`, `basis`;
  optional `symbol`.
- `## repetition <explained-in-step>`; required `description`, `sites` (space-separated
  `<file-id>:<side>:<first>-<last>` locations).
- `## thread <alias> <status>`; required `reason` for every thread listed in the job.

All ranges are inclusive and one-based. References must exist at the pinned commit.
Enums (literal values, never translated):
- Step kind: `modified`, `added`, `moved`, `renamed`, `deleted_site`, `context`.
- Annotation type: `behaviour`, `consequence`, `flag`, `note`.
- Flag level: `red`, `yellow`.
- Basis: `fact`, `inference`.
- Risk: `low`, `medium`, `high`.
- Origin: `introduced`, `pre_existing`, `unclear`.
- Blocking: `no_action_proposed`, `blocks_this_pr`, `follow_up`.
- Thread status: `still_valid`, `stale`, `already_addressed`.

All existing coverage, wording, evidence, annotation, assessment and thread rules apply.

## Complete example

This example assumes `h1` is the flow's planned head hunk in `p1`, lines 10–12,
with no listed GitHub threads. No extra file declaration or repeated path is needed.

```text
# flow f1

## introduction
problem_before: An empty queue waited indefinitely for a message.
change: Return when the caller closes the queue.
example_before_after: Before: close then wait. After: close then return Closed.

## step s1 h1
title: Return when the queue closes
called_by: The worker calls receive before dispatching a message.
decides: A closed queue ends the loop.
next: The worker returns to its caller.
why: This is the exit that replaces an indefinite wait.

### annotation behaviour fact 10-12
was: A closed queue waited for another message.
is: A closed queue returns Closed.
evidence: p1:head:10 p1:head:12

### annotation flag inference 11-11
level: yellow
text: Potential lost message on close:
scenario: <<END
A producer sends a message while the worker waits in receive.
The caller closes the queue before the worker reads it.
receive returns Closed; the sent message is dropped without an error.
END
not_checked: Not reproduced; no test was run.
evidence: p1:head:11

### assessment
risk: low
reason: The change affects only the closed-queue branch.
```

The parser reconstructs the canonical JSON document. Plan, PR context and Ask still
use schema-constrained JSON.
