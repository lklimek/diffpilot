# ADR-018: Jobs without a pull request — `JobTarget` and recreated job tables

- Status: accepted (owner allows a fresh database at the v1.1 deployment, OD8, 2026-11-18); a change request to
  frozen contracts (plan §4.3), implemented by WP11-JOB (`docs/v1.1-dashboard-plan.md`).

## Context
ADR-017 runs the dashboard judgement as a runner job. Every v1 job belongs to an analysis:
`JobSpec.analysis_id`, `pull_request_id` and `repo` are required (`crates/contracts/src/job/spec.rs`);
`JobRequest` carries `pr_repo` and a pipeline `JobStage` (`server/src/jobs/executor.rs`); the runner always
fetches and checks out `spec.repo` and saves the session under `pull_request_id` (`runner/src/execute.rs`
`prepare`, `run_session`); the `jobs` table has `analysis_id NOT NULL`, a `kind` CHECK, a `stage` CHECK tied to the
kind and triggers joining `analyses` (`crates/db/migrations/0001_init.sql`). SQLite cannot relax `NOT NULL` or a
CHECK in place.

## Decision
1. **Contracts.** `JobKind::JudgeWork` (`judge_work`). `JobSpec` replaces `analysis_id`, `pull_request_id` and
   `repo` with `target: JobTarget`:
   `PullRequest { analysis_id, pull_request_id, repo: RepoCheckout }` | `Dashboard { refresh_id }`.
   `JobRequest.pr_repo` becomes `Option<RepoRef>` (no installation token when absent); `JobStage` gains
   `Judge(refresh_id, batch)` with key `judge:<refresh>:<batch>`. Exhaustive matches make every analysis-only site
   a compile error.
2. **Runner.** For `Dashboard` targets `execute` creates the job directory and runs `provider.run` there: no
   mirror fetch or checkout, no session restore or save (session files stay in the job directory and are removed
   with it), no MCP servers, no skills.
3. **Server.** Claim skips session, context pack, installation-token minting and the MemCan endpoint for
   `Dashboard` targets; queue followers publish dashboard progress instead of analysis summaries; results update
   the LLM status as for every job; `cancel_analysis` ignores dashboard jobs; `select_model` uses the stored
   `judge_effort` for `JudgeWork`.
4. **Database.** A new append-only migration `NNNN_jobs_targets.sql` (ordinary transaction) drops
   `installation_tokens`, `job_events` and `jobs` (children first) and recreates them: `jobs.analysis_id`
   nullable, `refresh_id` → `dashboard_refreshes(id) ON DELETE CASCADE`,
   `CHECK ((analysis_id IS NULL) <> (refresh_id IS NULL))`, `kind` + `judge_work`, `stage` + `judge:%`; the five
   indexes and both triggers recreated; a partial unique index for one live judge job per (refresh, stage). Job
   history is discarded: the deployment starts from a fresh database anyway (OD8), and development databases lose
   only queue history. The migration applies cleanly to any v1 database (test). No data-preserving rebuild.
5. **Credential serialisation unchanged.** `jobs_one_running_per_credential` covers every job, so judge and
   analysis jobs never run together on one credential (ADR-003).

## Alternatives considered
| Alternative | Why not |
|---|---|
| Data-preserving rebuild (`no_tx`, foreign keys off, copy) | not needed with a fresh database (OD8); more migration risk for nothing |
| Second queue table for dashboard jobs | duplicates claim, lease, events, results and pause logic (`db/src/repo/jobs.rs`); credential exclusivity across tables needs triggers |
| Fake analysis/PR rows for dashboard jobs | corrupts the domain model |
| `Option` fields in `JobSpec` | same churn as an enum, allows invalid combinations |
| Separate claim response type | changes the runner protocol and doubles the runner core (brief §5 item 3) |

## Consequences
- One CCR commit touches v1 core files (contracts, jobs repository, claim, queue, runner route, runner execute,
  `select_model`, settings); scheduled in a quiet window (plan §H.2.5).
- The v1.1 deployment starts from a fresh database and an empty runner session store (runbook, WP11-DOCS).
- A later `ContainerExecutor` gets non-PR jobs for free: the target travels in the spec.
