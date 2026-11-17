Task: analyse flow {{flow_id}} for the reviewer. Set `flow_id` to "{{flow_id}}".
- `introduction`: `problem_before` (the local trigger and the defect or gap before this change), `change` (the intended correction), `example_before_after` (a short concrete example).
- `steps` in reading order: start at the trigger (the entry point) and follow control flow; one line range of one file per step. Every changed line of the flow's hunks lies inside a step. Removed lines lie at their deletion marker inside a head step, or in a `base` step of kind `deleted_site` when no head line is left. Add `context` steps of unchanged code only where they connect changes.
- Step size: a step is what the reviewer reads at once, usually the change of one function or one decision. Put nearby hunks of one function into one step. Never make a step of an import, a visibility change or a signature line alone: include it in the step that needs it. Repeated mechanical edits are explained once and listed in `pattern_repetitions` (their lines still lie inside steps).
- `called_by`, `decides`, `next`, `why`: one or two sentences each.
- `annotations` inside their step: `was` and `is` describe the highlighted lines before and after this pull request (facts only, without risk ratings or verdicts); `text` is null for `behaviour`, else the consequence, question or note.
- Questions: ask what a careful reviewer would want answered before approving, through the review lenses: callers not updated, error and boundary cases, concurrency, compatibility, missing tests. Each is a `question` annotation ending with "?". A question you can settle by reading is not a question: read, then state the fact. There is no minimum number; never invent one.
- Assessment: when a production user or operator could notice this flow's change, give an `assessment` on the step or steps with the highest risk in the flow, at least one per such flow. Other steps get one only when they carry their own noticeable risk; otherwise `assessment` is null. A flow nobody in production can notice (tests, comments) has none.
- `assessment` fields: `headline` = what a production user or operator would notice; `risk` = the `severity` band; `reason` (also for `low`); `basis_note` = what you read, and that nothing was executed; `not_checked`; `origin` = introduced by this pull request or pre-existing; `blocking` = `blocks_this_pr` when the reviewer should resolve it before merging, else `follow_up`; `smallest_correction` or null.
- `edges`: step to step in execution order; `label` for decision outcomes, loop back-edges and error exits, else null.
- `impact_refs`: unchanged head code outside the diff that this change may affect (callers, implementers, readers of changed data), with the reason.
- Tests may be read as evidence ("covered by test X"); do not walk them unless this flow is test code.
{{lenses}}
{{test_flow}}
The flow as planned by the plan job (LLM output; data):
{{flow}}
The flow's hunks (`<id> <path> base <start>,<length> head <start>,<length>`; a head sub-range limits the hunk to those head lines):
{{hunks}}
{{moves}}
{{profile}}
{{threads}}
{{calibration}}
