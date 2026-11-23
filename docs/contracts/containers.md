# Container runtime contracts (v1.3 common W1)

The Rust source in `crates/contracts/{src/runtime.rs,src/launcher.rs,src/gateway.rs}` is authoritative for the
provider-neutral wire shapes. JSON Schemas are generated in `schemas/runtime/`, and public/common TypeScript
in `web/src/api/generated/types.ts`. Codex JSON-RPC shapes remain local to `runner::provider`.

| ID | Contract | Enforcement owner |
|---|---|---|
| CCR1 | CS1 thread cwd `/work`; checkout `/work/repo`; context `/work/flow-context.json`; output `/work/out/flow.md`; repairs restore that same file. Review input `/work/pr-data/`, output `/work/out/report.json`. Inputs/outputs reset per job. | W3/W4/W5/W8 |
| CCR2 | Immutable analytic/ask/verify/judge thread kind. Full fixed config on every start/fork/resume. Analytic MemCan read/write; Ask read-only; verify/judge none. A loaded thread never changes kind. | W4/W8 |
| CCR3 | `LastContext` is separate from billable usage. Persist its generation and the working seed's `AskPreflight`. Fresh parent context above 80k requires one standalone compaction; only successful `turn/completed` confirms done. Missing/stale metadata requires recovery. A pending/uncertain preflight never permits an answer. | W4/W8 |
| CCR4 | CS1 runner setup may execute commands; CS2 setup/cleanup uses only filesystem RPCs. Judge has strict JSON output; any model tool item fails the job. | W3/W4/W8 |
| CCR5 | Usage is the sum of per-turn deltas of cumulative totals, including standalone compaction and answer turns. Exclude inherited fork totals. Keep input (including cached), cached input and output separately; last-context is never a usage counter. | W2/W4/W8 |
| CCR6 | Exact `invalid_json_schema` code in failed-turn `error.message`/embedded backend JSON maps to `InvalidJsonSchema`, even with error-info `other`. It is non-retryable and cannot enter model repair. Completed output still gets server schema validation. | W4 |
| CCR7 | Attach, resume existing source/target with full fixed config, then act; keep attachment through status/fork/compaction/turn. Threads unload after 60s without subscribers. Relay/frame loss means worker_lost: stop container before another turn. Keep attempt/lease/generation fencing. | W4/W8 |
| CCR8 | `relink_needed` distinguishes `terminal` from `quarantined`; both become gateway 403 `credential_unlinked`, job `credential_relink`, without automatic retry. DTOs contain no real credentials. | W2/W6 |
| CCR9 | `UnlinkCredential` alone authorizes upstream revoke. `RevokeKeys` rotation/idle-stop/retirement is strictly local. Gateway owns encrypted credentials, generation CAS, durable in-flight markers and persist-before-use outside DB. Auth tests use fakes exclusively. | W2/W7 |

## Plan-only credential seam (KL1-A)

`CredentialStatus` and the database store connection state, safe relink reason, optional provider label/plan,
payer pause and last model call, with singleton `mode: chatgpt_plan`. `LinkCredential` accepts only that mode;
unknown fields (including API key, token and base URL) are rejected. Migration 0030 constrains mode and state.
W2 implements gateway device linking and W6 exposes `/settings/codex[/link]` with a disclosure that linking may
log Codex out elsewhere. No paid fallback, secret export or guarantee of session isolation is authorized.
A virtual key is a `Secret` delivered only to the runner for its tmpfs file; it is not an upstream credential.

## Persistence and cut-over

`0030_v13_containers.sql` starts with a temporary `CHECK (n = 0)` table receiving the count of **each** rebuilt
and dropped table: llm_credentials, installation_tokens, job_events, jobs, comment_assessments and working_sessions. A non-zero
count aborts the migration transaction before any destructive statement. Start with a new empty database;
never empty an existing production database to bypass this guard. W10 imports only non-secret settings.

Jobs store payer and PR container identity; partial unique indexes enforce one claimed/running job per
(PR,payer), or per payer for judge_work. Target triggers keep a job's explicit PR equal to its analysis/pass PR.
The two shapes are independent: a payer's judge worker and PR worker may each own a job.
`analytic_threads` is keyed by (PR,user), and last-context generation must equal its analytic generation.
Working sessions are keyed by (PR,user,kind), preserving separate analytic Ask and review Ask threads.
This requires rebuilding the empty working_sessions table as well. Working preflight is bound to seed, generation and immutable kind; repository acknowledgement additionally
matches thread id. No `comment_reports` table or legacy credential adoption exists.

`Db::container_sessions()` exposes generation-CAS checkpoints and fenced preflight acknowledgement.
`Db::comment_sources()` keeps cleaned-text versions, including zero-point sources, and a first-read marker.
`Db::llm_credentials().put_status/status` stores only common gateway status. Callers retain the worker lease
through seed/preflight/answer; a pending marker survives interruption and requires reconciliation.

## Review report hand-in

`PrData` maps to manifest.json, threads.json, review-comments.json, reviews.json, comments.json,
conventions/, ids.json and scope.json, matching claudius 13.5.0 `460e5fd`. Manifest includes title/body,
base_ref/base_sha and optional delegates. Comment/review files are arrays retaining GitHub timestamps, commit
and review IDs; ids.json preserves source_kind. Untrusted bodies retain details content after cleaning.
Only the PR author's own text is author context; tenant membership does not make someone context.
Commands, gate/status comments and pending reviews are ignored. Latest-round bot rules remain server-owned.

`agent::comment_report` owns read-side serde types for claudius 4.2.x, `comment_check`, `host_supplied`.
Unknown fields are tolerated and discarded. Envelope rows remain JSON values for independent row decoding:
a malformed row is dropped and logged rather than failing the whole pass. W5 enforces head/PR/scope,
quote grounding and ID reuse, decision authority and target semantics. Severity floats and merge_class are
not stored or displayed. `report_type` is in metadata; duplicate `merged_into` and relay `on_behalf_of`
survive decoding. Native `VerifyCommentsInput`/`VerifyCommentsOutput`, strict schema and adapter remain
available for `prompt_v4`; W5 selects either adapter without weakening its validation contract.
This common contract does not switch the live server pipeline.

The runner's existing exec/credential-report plumbing is removed by W9 after W4/W8 integrate the new runtime.
An empty event batch remains the heartbeat and carries no credential status. `retire` in claim responses is
the common PR-container retirement instruction. Existing attempt tokens and self-fencing remain unchanged.


## Frozen assessments and UTC admission (CCR11–CCR13)

`assessment::AssessmentConfig` defaults to `claudius_host`, no selected PRs and no deadline. Pairing is frozen
when the PR is in `ab_prs` or admission precedes `ab_until`; settings alone never schedule work. JobSpec and
JobResult carry `AssessmentRunRef` (logical pass/method), with the enclosing job attempt as the fence.
`api::assessment` defines Settings and read-only comparison DTOs; HTTP orchestration belongs to W5/W6.

`Db::assessments().admit` validates the current usable credential and atomically reserves one of 80 UTC daily
slots shared across the payer's tenant memberships, freezes the snapshot/configuration and inserts one or two method jobs. Jobs must use the same payer,
pass, PR, head/base and model, with the primary first. A repeated payer/request key returns the original pass,
even after midnight, config changes or restart. Rejected requests use no slot; admitted failures retain theirs.
`AssessmentAdmissionFailure::AssessmentDailyLimit { resets_at }` and `AssessmentQuota` expose safe quota status.

`runs` returns method job IDs separately from the retained legacy `comment_passes` APIs.
`begin_attempt` checks the current queue attempt; `finish_attempt` accepts it once and deduplicates metering
per job/attempt. `record_attempt_usage` can add late metering from a lost attempt without accepting its output.
`retry_method` requeues only a retryable failed method, keeping the pass and snapshot. Secondary output writes comparison rows
only; composite foreign keys reject secondary or cross-pass primary attribution. The existing
`add_assessment` repository infers attribution from its job and requires the accepted primary attempt. Comparison rows retain
native/normalized verdicts, nullable proposals, source/version/quote, explicit missing/unmatched/duplicate
state and direct canonical links. Raw reports are not stored. Callers scrub/normalize output before persistence.

`record_owner_decision` reads the actual diffpilot decision and supersession revision through a primary item;
model attribution is never comparison truth. Head, frozen point hash and source version must match; actual decisions snapshot the source version at write time. Read snapshots select the latest
actual owner revision at the requested cutoff; unmatched secondary rows remain unscored. W5 owns alignment,
primary effects, sequential orchestration and final report summaries; W6 owns the Settings/Comments controls.
