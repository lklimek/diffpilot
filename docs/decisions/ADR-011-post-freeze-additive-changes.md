# ADR-011: Additive changes after the contract freeze

- Status: accepted (coordinator, 2026-11-16, stage-2 change requests of WP2-INGEST and WP2-JOBS)
- Source: ADR-010 (freeze; every contract change is a coordinator-approved change request, plan §4.3).

## Rule: the analysis base is the merge base
The analysed diff is GitHub's three-dot comparison: from the merge base of the PR's base tip and head to the
head. The pipeline fetches both commits, computes the merge base and stores it as the analysis base before any
job is submitted (`pipeline::prepare`). Preparing again is a no-op (the merge base of a merge base and the head
is that merge base). An analysis whose stored base is not the merge base is not ready: `lines` and `diff` answer
409 `analysis_not_finished` (`docs/contracts/http-api.md` "Analysis base").

## Changes
All three are additive; no existing field, signature or behaviour changed.

| Change | Reason | Effect |
|---|---|---|
| `DeletedBlock.total_lines: u32` (`contracts::api::code`, `72d8147`) | A deleted block longer than the line budget (`RangeQuery::MAX_LINES` = 2000 removed lines per response, shared by the blocks in order) was cut without a sign. | The truncation is visible: `lines.len() < total_lines` marks a cut block, and a client reads the rest with `side=base` from `base_start + lines.len()`. Schemas, TS types and the seed fixtures carry the field. |
| `Analyses::set_base_sha(id, sha)` (`crates/db`, `173e5cb`) | The pull request row holds the base tip; the pipeline needs to store the merge base (rule above). | Replaces the base while the analysis has no job and no document; afterwards `Conflict` (jobs and documents were made against the stored base); unknown analysis `NotFound`. |
| `JobExecutor::cancel_analysis(analysis)` (`server::jobs`, `135f245`) | Rule A3: the pipeline talks only to `JobExecutor`, yet an ended analysis must cancel jobs submitted before a server restart, which no in-memory handle knows. | Requests cancellation of every queued, claimed, running or paused job of the analysis (`QueueExecutor`: `Jobs::list_for_analysis` + `Jobs::request_cancel`). The pipeline calls it once when an analysis fails or is cancelled and never reads the jobs table (`runner-protocol.md` "End boundary"). |

## Consequences
- Generated schemas and TypeScript types changed only by `total_lines`; the drift test guards them.
- Further contract changes still need a change request (ADR-010).
