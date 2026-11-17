# ADR-015: Defer parallel flow analysis

Status: accepted, owner decision, 2026-11-17. No parallel scheduler or runner change is authorised here.

## Context

The owner compared dashpay/platform#5889 (about 4,800 changed lines), using the same model and effort:
interactive review took about 14 minutes and 25,000 output tokens; this product took over 80 minutes and
184,000 output tokens for planning, PR context and 15 of 20 flows. This is an owner-reported comparison,
not a controlled benchmark. Output generation dominates; concurrency would not reduce the output volume.
The first changes are earlier access to accepted results, on-demand flow priority, optional background
work and concise low-risk assessments. Effort remains one installation-wide setting; steps are not capped.

## Decision

Keep one job at a time per PR and credential. Defer parallel flow jobs until their isolation and credential
requirements can be proved. A future experiment starts at k=2, never by disabling existing exclusions.

The source assessment, `reviews/parallel-flows-analysis.md` (2026-11-17), proposes:

- One runner-owned credential refresh authority, serialising the complete exchange and durable publication.
  Jobs receive disposable access-token-only snapshots, never refresh tokens. Expiry and 401 recovery need
  bounded runner-controlled handling; read-only copies of full credentials are not a safe substitute.
- A frozen seed after accepted planning and PR context, separate per-flow archives and frozen fallback
  context packs. Repairs resume their own flow. Checkpoint publication must fence stale attempts and rounds;
  Ask retains private PR/user archives. Storage bounds and retention must cover multiplied archives.
- A bounded scheduler, initially k=2, with independent leases, cancellation, repair and restart recovery.
  Planning remains serial. Claim exclusions and DB indexes/triggers must admit only compatible flow forks.
- Distinct agent UIDs and slot-scoped supervisor operations, cleanup and quarantine. Mirror mutation and
  skills refresh also need coordination. Memory, disk and inode use must be measured on the 15 GiB host.

These are coupled changes to ADR-003, ADR-004 and ADR-013, not a concurrent-futures substitution.
Current evidence was checked in `crates/db/src/repo/jobs.rs` (claim exclusions), `0001_init.sql`
(analysis/credential indexes and PR triggers), `crates/runner/src/supervisor/protocol.rs` (exclusive helpers
and global agent-UID sweep), `provider/codex/credential.rs` (master/copy settlement), and
`crates/server/src/pipeline/context.rs` (stored-flow context packs). The source's recommendation to wait
for a complete Tour is superseded by the owner's lazy-review decision; concurrency remains deferred.

## Alternatives not selected

Codex sub-agents inside one job stay disabled (`SUB_AGENTS_DEFAULT` and `multi_agent = false`). Delegation
would leave scheduling, per-flow progress, retry and cancellation inside one agent turn; it does not provide
the product's independent verified flow checkpoints. Token volume and context duplication still require
measurement. This is a design inference, not a measured speed comparison.

A long-lived Codex app-server per analysis is also deferred. The inspected assessment
`reviews/context-session-analysis-2.md` describes a possible typed turn protocol, but retained processes do
not make model context free. Current per-job homes, supervisor sweep, credential settlement and GitHub
installation-token revocation require a different lifecycle and safe checkpoint/configuration updates.
Current code still prepares each job separately (`runner/src/execute.rs`, `provider/codex/isolated.rs`);
keeping a process alive alone does not remove that work. No production benefit has been measured.

## Smallest experiment before reconsideration

In an isolated test setup, first prove refresh-only credential renewal and access-only exec/resume, including
expiry, 401, refresh persistence failure and restart. Then compare the same frozen seed and 4–6 representative
flows at k=1 and k=2 with identical model, global effort, prompts and output format. Measure wall time,
output/cached-input tokens, repairs, limits, peak memory and archive bytes; compare accepted coverage and
assessment quality. Prove that finishing/cancelling one slot cannot read, kill or corrupt the other.
Any speed-up remains a hypothesis until measured. A separate sequential exec/resume versus app-server
experiment should isolate startup savings, include real token replacement/revocation, and reverse run order
to expose cache warmth. Neither experiment is part of this implementation.
