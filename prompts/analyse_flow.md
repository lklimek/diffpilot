Task: analyse flow {{flow_id}} for the reviewer. Set `flow_id` to "{{flow_id}}".
- `introduction`: `problem_before` (the local trigger and the defect or gap before this change), `change` (the intended correction), `example_before_after` (a short concrete example).
- `steps` in reading order: start at the trigger (the entry point) and follow control flow; one line range of one file per step. Every changed line of the flow's hunks lies inside a step. Removed lines lie at their deletion marker inside a head step, or in a `base` step of kind `deleted_site` when no head line is left. Do not create steps made only of unchanged context: describe callers and connecting code in `called_by`, `next`, evidence and `impact_refs`.
- Step size: a step is what the reviewer reads at once, usually the change of one function or one decision. Put nearby hunks of one function into one step. Never split one statement, call expression or its arguments across steps; include the complete statement around a change. Never make a step of an import, a visibility change or a signature line alone: include it in the step that needs it. Repeated mechanical edits are explained once and listed in `pattern_repetitions` (their lines still lie inside steps).
- `called_by`, `decides`, `next`, `why`: one or two sentences each.
- `annotations` inside their step: for `type: behaviour`, `was` and `is` are nonempty before/after facts, without risk or verdicts; omit `text`. For `consequence`, `question` and `note`, omit `was` and `is` and put the content only in nonempty `text`. Never copy behaviour into another annotation. A behaviour annotation and a question may share lines.
- In the tagged file omit empty optional fields. Use the short file/hunk ids from `sr-check catalog out/flow.md`; do not repeat paths or the locations/kind of a planned hunk. Prose and code are raw text (use a heredoc for multiline values).
- Questions: ask what a careful reviewer would want answered before approving, through the review lenses: callers not updated, error and boundary cases, concurrency, compatibility, missing tests. Each is a `question` annotation ending with "?". A question you can settle by reading is not a question: read, then state the fact. There is no minimum number; never invent one.
- Assessment concerns unresolved risk this CHANGE introduces, worsens, exposes or leaves relevant to its intent: what may break or remain wrong after merging. A severe pre-existing bug that this PR fixes belongs in `introduction.problem_before` and PR-context `problem`, never in the change's risk band. `origin: pre_existing` is only for a still-present problem, not the fixed one.
- Apply the `severity` skill's production impact, likelihood and PR-scope guidance to that unresolved risk, against the project profile. Output only this schema's bands (`low`, `medium`, `high`); never skill report fields, numeric scores or merge-class verdicts. Do not rate the size or benefit of the fix as its risk.
- Every step requires an `### assessment`, including low-risk and context steps. Rate what the CHANGE may break or leave wrong, with its band and one sentence reason. For low, write only `risk` and `reason`; omit the other assessment fields. Above low, write the full assessment with evidence basis and concrete checks not performed. The flow risk is the highest step band. A rating is not a defect claim: never invent a defect or action. For a full assessment with no blocking action or follow-up proposed, use `blocking: no_action_proposed` and omit `smallest_correction`; low needs only the bounded exposure in its one-sentence reason.
- Full `assessment` above low: `headline` = the unresolved production effect; `risk` = its band; `reason` = why (also for low); `basis_note` = evidence read and that nothing was executed; `not_checked` = concrete checks not performed, not missing agent capabilities. `blocking: blocks_this_pr` requires a concern to resolve before merge; `follow_up` requires a named action in `smallest_correction` (what to check/change and where), never a default for "nothing blocks". A non-low band needs a concrete `smallest_correction` or a question annotation on this step with evidence and ending in "?". No placeholder corrections such as "none given", "N/A" or "none"; omit `smallest_correction` when no correction is proposed.
- `edges`: step to step in execution order; `label` for decision outcomes, loop back-edges and error exits, otherwise omit it.
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
