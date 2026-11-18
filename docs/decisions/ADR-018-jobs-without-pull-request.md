# ADR-018: Jobs without a pull request — `JobTarget` and the `jobs` table rebuild

- Status: proposed (architect, 2026-11-18); a change request to frozen contracts (plan §4.3) for v1.1
  (`docs/v1.1-dashboard-plan.md` WP11-JOB). Gate: spike S11-5.

## Context
ADR-017 runs the dashboard judgement as a runner job. Every v1 job belongs to an analysis:
`JobSpec.analysis_id`, `pull_request_id` and `repo` are required (`crates/contracts/src/job/spec.rs`);
`JobRequest` carries `pr_repo` and a `JobStage` of the pipeline (`server/src/jobs/executor.rs`); the runner always
fetches and checks out `spec.repo` and saves the session under `pull_request_id` (`runner/src/execute.rs`
`prepare`, `run_session`); the `jobs` table has `analysis_id NOT NULL`, a `kind` CHECK list, a `stage` CHECK tied
to the kind, and triggers joining `analyses` (`crates/db/migrations/0001_init.sql`). SQLite cannot relax
`NOT NULL` or a CHECK in place.

## Decision
1. **Contracts.** `JobKind::JudgeWork` (`judge_work`). `JobSpec` replaces `analysis_id`, `pull_request_id` and
   `repo` with `target: JobTarget`:
   `PullRequest { analysis_id, pull_request_id, repo: RepoCheckout }` | `Dashboard { refresh_id }`.
   `JobRequest.pr_repo` becomes `Option<RepoRef>` (no installation token when absent); `JobStage` gains
   `Judge(refresh_id, batch)` with key `judge:<refresh>:<batch>`. Exhaustive matches make every analysis-only site
   a compile error, so none is missed.
2. **Runner.** For `Dashboard` targets `execute` creates the job directory and runs `provider.run` there: no
   mirror fetch or checkout, no session restore or save (session files stay in the job directory and are removed
   with it), no MCP servers (access is empty), no skills. `ask`-route validation is unchanged.
3. **Server.** Claim skips session, context pack, installation-token minting and the MemCan endpoint for
   `Dashboard` targets; queue followers publish dashboard progress instead of analysis summaries; result handling
   updates the LLM status as for every job. `cancel_analysis` ignores dashboard jobs.
4. **Database.** A separate migration `NNNN_jobs_targets.sql` registered with `no_tx = true` (sqlx
   `Migration::new(.., no_tx)`) rebuilds `jobs` by SQLite's documented procedure: `PRAGMA foreign_keys=OFF`;
   `BEGIN IMMEDIATE`; create `jobs_new` with `analysis_id` nullable, `refresh_id` → `dashboard_refreshes(id) ON
   DELETE CASCADE`, `CHECK ((analysis_id IS NULL) <> (refresh_id IS NULL))`, `kind` + `judge_work`, `stage` +
   `judge:%`; copy rows; drop `jobs`; rename; recreate the five indexes and both triggers; add a partial unique
   index for one live judge job per (refresh, stage); `PRAGMA foreign_key_check` must return nothing; `COMMIT`;
   `PRAGMA foreign_keys=ON`. `job_events` and `installation_tokens` keep their rows and references (checked by a
   test on a populated database).
5. **Credential serialisation unchanged.** `jobs_one_running_per_credential` still covers every job, so judge
   jobs and analysis jobs never run at the same time on one credential (ADR-003).

## Alternatives considered
| Alternative | Why not |
|---|---|
| Second queue table for dashboard jobs | duplicates claim, lease, events, results and pause logic (`db/src/repo/jobs.rs`, ~2,100 lines); credential exclusivity would need cross-table triggers |
| Fake analysis/PR rows for dashboard jobs | corrupts the domain model; analysis triggers, retention and UI would see them |
| Make the three fields `Option` in `JobSpec` | same churn as an enum, but allows invalid combinations |
| Separate claim response type for dashboard work | changes the runner protocol and doubles the runner core (`execute` stays one function, brief §5 item 3) |
| No LLM judgement (deterministic only) | kept as the fallback if S11-5 fails (owner question Q11-9) |

## Consequences
- One CCR commit touches v1 core files (contracts, jobs repository, claim, queue, runner route, runner execute,
  `select_model`); scheduled in a quiet window (plan §H.2.5).
- Migration risk (G-BRICK) is isolated in one `no_tx` file with a populated-copy test; the earlier dashboard
  tables are in a separate additive migration.
- A later `ContainerExecutor` gets non-PR jobs for free: the target travels in the spec.
