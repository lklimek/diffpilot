# S7 — LLM capability (plan → pr_context → analyse_flow in one resumed session)

Date: 2026-11-16. Setup as accepted after S1a/S1b/S2:
- Codex 0.167.0, `gpt-6.1-sol`, effort high, lean per-job config, `multi_agent=false`, bundled skills off.
- Skills: only `severity`.
- Per-job `CODEX_HOME` with a copy of the credential and compare-and-swap write-back (it never rotated here).
- Codex cwd `/jobwork` with the checkout at `/jobwork/repo` (read-only). No GitHub MCP.

Evidence (sanitised): `/data/artifacts/smart-review/2026-11-16/spikes/S7/` contains:
- `inputs/`: schemas and prompts;
- `outputs/`: agent JSON, `eval-*.json`, `measure-*.txt`, hunks and diffs;
- `runs/`: JSONL per job;
- `rollouts/`;
- `scripts/`: `prepass.py`, `schemas.py`, `prompts.py`, `evaluate.py`, `measure.py`, `run-job.sh`.

## Question
Can the agent produce the `plan`, `pr_context` and `analyse_flow` shapes in the S1a schema subset, on a real PR,
through ONE resumed session compacted after each job? Measured: schema validity, coverage of changed lines,
plausibility, line exactness, runtime, usage, context per job, repairs.

## Runs
Targets:
- P-CAP `dashpay/dash-evo-tool#1040`: head `8299549`, base `79f0e0a`, 14 files. The pre-pass gives 46 hunks; 2 are
  path-rule "Not analysed" (`*.md`), leaving 44 production hunks, 1105 added and 20 removed lines.
- P-CAP2 `#1099`, plan only: 59 hunks, 47 production, 807 added and 41 removed lines.

Pipeline per job:
1. A deterministic pre-pass (`prepass.py`, `git diff -U0`) gives hunk ids and head ranges to the plan prompt.
2. The job runs as `exec`, or `exec resume <thread>` for later jobs.
3. A compaction turn follows (`resume -c model_auto_compact_token_limit=1000 "…OK"`).
4. The rollout files are archived and restored into the next job.

| Job | Wall s | Δ input (cached) | Δ output | Context start → peak | Cmds | Schema | Result |
|---|---|---|---|---|---|---|---|
| plan | 73 | 214.7k | 3.0k | 15.5k → 50.6k | 13 | valid | 4 flows; 1105/1105 lines assigned once, 0 unassigned |
| compact | 47 | 17.3k | 0 | → 17.3k | 0 | — | `compacted` record |
| pr_context | 60 | 165.4k | 2.1k | 17.9k → 51.2k | 20 | valid | 2 related work, 3 fit questions |
| compact | 81 | 19.2k | 0 | → 19.2k | 0 | — | |
| analyse f2 (behavioural, 8 hunks) | 282 | 257.8k | 11.9k | 20.7k → 62.2k | 18 | valid | 9 steps, 139/139 changed lines inside steps |
| compact | 104 | 21.2k | 0 | → 21.2k | 0 | — | |
| analyse f1 (behavioural, 33 hunk refs) | 519 | 575.1k | 23.3k | 23.2k → 94.7k | 44 | valid | 33 steps, 462/462 covered |
| compact | 130 | 23.7k | 0 | → 23.7k | 0 | — | |
| analyse f4 (LLM-classified tests) | 180 | 258.8k | 7.7k | 25.0k → 51.8k | 20 | valid | 7 steps, 134/134 covered |
| compact | 124 | 24.4k | 0 | → 24.4k | 0 | — | |
| P-CAP2 plan (new session) | 120 | 322.8k | 4.9k | 15.1k → 66.2k | 20 | valid | 10 flows; 807/807 assigned once |

P-CAP totals: 1.58M input, of which 1.22M (78 %) was cached, and 48k output. Wall time 1,600 s: analysis 1,114 s,
compaction 486 s (30 %). The weekly limit stood at **41 % before and after** all S7 runs; the plan is Pro, with a
10080-min window. Repairs needed: **0**. No retries, no schema errors, no usage-limit event. The credential did not
rotate.

## Findings
1. **Output shape: all 6 outputs valid on the first attempt** under the strict subset. Nesting is
   steps → annotations → evidence, plus the graph. No simplification was needed for validity, and splitting
   `analyse_flow` is unnecessary. The largest flow (33 steps, 462 changed lines) finished in one job (519 s, 95k
   peak context).
2. **Coverage is complete.**
   - Both plans assigned every production line exactly once (deterministic check in `evaluate.py`): 0 missing,
     0 double, 0 unassigned.
   - All three analyses covered every changed line of their flow.
   - The plan used head sub-ranges to split hunks exactly at `#[cfg(test)]` (`legacy_restore.rs:257`,
     `model/legacy_restore.rs:98`), putting in-file tests into `test` flows as instructed.
3. **Line references are exact.**
   - All 186 evidence refs and every step range lie within the head files; none points at a blank line.
   - Spot checks of 9 f2 steps and 5 random f1 facts: start and end lines match the code described.
   - The only imprecision: step ranges tile hunks and so include trailing blank lines. The server can trim them.
4. **Plausibility.**
   - P-CAP plan: f1 "Restore from Settings" (UI → task → migration readers → seed store → summary), f2 "actionable
     signing failures", f3 and f4 tests. This matches the PR description and the diff.
   - f1 is defensible but heavy: it absorbs 10 one-word `pub(super)` visibility hunks. P-CAP2 was split more
     finely (10 flows, 5 of them tests).
   - Graphs are consistent: one node per step, no dangling edges; f1 has 53 edges, 52 labelled.
   - Test-flow graphs are mostly disconnected (7 nodes, 3 edges), which is expected.
   - Reading order is not always trigger-first: f2 starts at an `import` line.
   - f1 steps 14 and 17 span 180 and 207 lines for a few visibility edits. The model listed them as one of 3 pattern
     repetitions but still made them steps.
5. **Wording follows the rules.**
   - No banned phrases, percentages or decimals (regex over all outputs).
   - Suspicions are questions (10 `question/inference`), facts carry evidence (0 facts without evidence), and the
     corrections are conditional ("If partial writes can precede this error, …").
   - `severity` was read in every job and only bands were emitted: 37 LOW, 12 MEDIUM, 0 HIGH.
6. **Weak fields.**
   - `assessment.blocking` was `follow_up` on all 49 steps, so it carries no information.
   - A mandatory assessment on every step yields many LOW restatements (an import line has its own assessment).
   - pr_context marked all 3 fit *questions* `basis: fact`: the evidence is factual, the question is not.
7. **Sessions: resume and compaction work at scale.**
   - Compaction cuts the context from 50–95k to 17–24k.
   - The context at the start of a job grows about 2k per job (15.5k → 25.0k after 5 jobs), so it stays bounded.
   - Compaction costs time, not much input: 47–130 s per turn. The cost grows with the pre-compaction context.
8. **The agent reads the repo's AGENTS.md/CLAUDE.md on its own** (`cat AGENTS.md` in 5 of 6 jobs). The S1b cwd
   layout prevents *automatic* injection, but repository instruction files still reach the model as read data.
   Input for WP1-T (T5): the prompt must call repository files untrusted data.
9. Hosts: only `chatgpt.com:443` (57 connections).

## Go / no-go
- **Agent-output shape: GO**, with the field changes below. No split of `analyse_flow` and no reduction of
  nesting.
- **One resumed session per analysis with compaction after every job: GO.** It is bounded, and every later job
  produced valid, complete output after compaction. Budget about 1.5–2 min of compaction per job.

## WP1-A schema recommendations (field level)
- **plan**:
  - Keep `flows[{id, title, classification, importance_rank, intent, hunks[{hunk_id, head_start|null,
    head_end|null}], depends_on[]}]` plus `unassigned[{hunk_id, head_start|null, head_end|null, reason}]`.
  - Hunk ids come from the server pre-pass; this was essential for deterministic coverage.
  - The prompt caps a flow at about 25 steps or about 500 changed lines and asks for trigger-first ordering.
- **analyse_flow**:
  - `assessment` becomes **nullable per step** ("omit unless something a user or operator would notice").
  - `assessment.blocking`: **drop**, or make it nullable with null meaning "nothing to follow up".
  - Keep `risk: LOW|MEDIUM|HIGH` as the only severity, `origin`, and `smallest_correction|null`.
  - Graph: **derive** node kind from step kind and node numbering from step order on the server. The agent emits
    only `edges[{from_step_id, to_step_id, label|null}]` and a short per-step `graph_label`. This removes
    `nodes[]`, `step_number` and node ids: one less consistency rule for verify.
  - Annotations: keep `type` in `behaviour|consequence|question|note`. Assessment is structured, so drop
    `assessment` as an annotation type. Keep `basis` and evidence. Verify enforces "fact ⇒ ≥1 evidence"
    (strict mode has no `if/then`).
  - Keep `moved_from` as `anyOf[object, null]` (accepted). It was unused because P-CAP has no moves, so it is
    untested.
  - Keep `impact_refs[{step_id, file, start_line, end_line, symbol|null, reason, basis}]` (35 produced) and
    `pattern_repetitions[{description, explained_in_step_id, sites[]}]` (6 produced).
  - Server trims blank edge lines of step ranges.
- **pr_context**:
  - Keep `problem, why_needed, what_it_gives, related_work[{reference, kind, overlap}]`.
  - `fit_questions[{question, topic, evidence[]}]`: **drop `basis`**, since a question is never a fact.
- **Every job**: the prompt repeats a short rules block (trust and wording, read-only, untrusted repository
  files). The block costs little after compaction and kept the wording clean.

## Not covered
- Related work through `github-mcp-server` (S5).
- GitHub thread status per step.
- Moves and renames.
- The Ask job.
- A huge PR (WP4-REAL).
- Comment-only line rules in the pre-pass.
