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

The owner's 2026-11-23 decision supersedes the empty-database/guarded-0030 and D18 export/import cut-over.
`0030_v13_containers.sql` upgrades the existing database in place: SQLx's migration transaction deletes all
old analysis/runtime tables' rows and rebuilds the container tables with foreign keys enabled. A failure rolls
back both cleanup and schema changes; a fresh database follows the same migration path. Do not run an
additional purge or settings import. Migrations 0001–0029 remain unchanged; only unmerged 0030 is edited.

Except for watch runtime fields, the keep list preserves complete rows byte-for-byte, including stored GitHub
authentication, cookie/OAuth sign-in state, PAT hashes, channel/push secrets and existing configuration bookkeeping. No credential value is
read, decrypted, exported or logged by 0030. Repository/PR identity and metadata stay so watches keep their
original foreign keys. The only retained-table schema change removes the three legacy PR thread columns.
Codex credentials are the explicit exception: `llm_credentials` is rebuilt empty and plan-only (CCR10);
every user must link Codex again through the gateway. No legacy credential is adopted.

The following inventory covers every application table in 0001–0030; “clean” means empty after migration,
and “new” means created empty. Settings stored outside SQLite are untouched.

| ID | Table | Action | Reason |
|---|---|---|---|
| M01 | `access_tokens` | Keep | PAT identity, hashes, scopes and revocation state. |
| M02 | `agent_subscriptions` | Keep | PAT subscription capabilities and webhook/push configuration, including encrypted secret. |
| M03 | `analyses` | Clean | Old analyses, progress and payer-free runtime. |
| M04 | `analytic_threads` | New | Per-payer analytic thread checkpoints. |
| M05 | `assessment_attempt_usage` | New | New attempt metering. |
| M06 | `assessment_comparison_items` | New | New normalized A/B comparisons. |
| M07 | `assessment_config` | New | New assessment configuration; defaults apply until saved. |
| M08 | `assessment_daily_usage` | New | New atomic UTC quota counters. |
| M09 | `assessment_method_runs` | New | New independent method runs. |
| M10 | `assessment_owner_decisions` | New | New assessment owner-history attribution. |
| M11 | `assessment_passes` | New | New frozen assessment inputs. |
| M12 | `comment_assessments` | Clean | Drop/recreate empty with method/pass attribution. |
| M13 | `comment_carry_events` | Clean | Old decision carry history. |
| M14 | `comment_decision_undos` | Clean | Old decision undos. |
| M15 | `comment_decisions` | Clean | Old review-loop decisions and feed tasks. |
| M16 | `comment_effect_reports` | Clean | Old executor/idempotency reports. |
| M17 | `comment_effects` | Clean | Old effect claims, leases and outcomes. |
| M18 | `comment_item_skips` | Clean | Old item queue state. |
| M19 | `comment_items` | Clean | Old findings/items; add source/quote/duplicate columns to empty table. |
| M20 | `comment_passes` | Clean | Old comment assessment queue and quota history. |
| M21 | `comment_prs` | Clean | Old GitHub comment snapshots, purposes, round/item counters. |
| M22 | `comment_requests` | Clean | Old analysis comment request idempotency/cache. |
| M23 | `comment_sources` | New | New versioned source/read cache. |
| M24 | `controlled_owners` | Keep | Installation controlled owners. |
| M25 | `controlled_owners_changed` | Keep | Settings editor and timestamp. |
| M26 | `dashboard_hidden_mentions` | Keep | User visibility preference. |
| M27 | `dashboard_judgements` | Clean | Old ranking/judgement cache. |
| M28 | `dashboard_refreshes` | Clean | Old scans, snapshots, rankings and send counters (including 0029 automatic refreshes). |
| M29 | `dashboard_settings` | Keep | Linked logins, repositories/aliases, release branches, ignored checks, ownership policy, skip authors, lookback, labels, TODO/MemCan options, bots, Slack names, schedule and time zone. |
| M30 | `dashboard_snoozes` | Keep | Active and ended snoozes; unchanged schema. |
| M31 | `documents` | Clean | All analysis outputs, flows, context and snapshot/profile caches. |
| M32 | `feedback` | Clean | Old analysis feedback and MemCan write queue. |
| M33 | `flow_work` | Clean | Old flow queue, retries and failures. |
| M34 | `installation_tokens` | Clean | Drop/recreate empty; old per-job tokens are runtime data. |
| M35 | `job_events` | Clean | Drop/recreate empty; old attempt logs. |
| M36 | `jobs` | Clean | Drop/recreate empty with payer/container identity. |
| M37 | `llm_credentials` | Clean | Drop/recreate empty plan-only status; gateway relinking required. |
| M38 | `llm_selection` | Keep | Installation model, reasoning effort, Ask effort and judge effort. |
| M39 | `memberships` | Keep | Tenant membership. |
| M40 | `notification_channels` | Keep | Channel URLs/topics, enabled events and encrypted secrets. |
| M41 | `notification_deliveries` | Clean | Old user/decision delivery queue and retries. |
| M42 | `notification_sends` | Clean | Old delivery/rate-limit history. |
| M43 | `oauth_logins` | Keep | In-progress sign-in state and encrypted PKCE verifier. |
| M44 | `pr_watches` | Keep; reset runtime | Keep identity, since and paused/ended state; clear probe, last_probe_at, last_auto_pass_at/questions, pass_due_at and notified_for; other states become active. The next probe rebuilds review-loop data and allows notifications for the same round; automatic assessments use quota and require Codex relinking. Schema unchanged. |
| M45 | `pull_requests` | Keep; schema change | Keep identity/metadata; drop agent_thread_id, analytic_generation and session_state. |
| M46 | `questions` | Clean | Old Ask/review questions, answers and threads. |
| M47 | `repos` | Keep | Repository identity and GitHub installation mapping for retained PRs. |
| M48 | `review_states` | Clean | Old analysis review progress. |
| M49 | `sessions` | Keep | Browser sign-in sessions, token hashes and CSRF state (not agent threads). |
| M50 | `tenant_allowed_logins` | Keep | Allow-list and bound GitHub identities. |
| M51 | `tenants` | Keep | Tenant identity. |
| M52 | `user_events` | Clean | Old browser events and round notification deduplication. |
| M53 | `user_tokens` | Keep | Stored encrypted GitHub access/refresh tokens and refresh version. |
| M54 | `users` | Keep | User IDs, linked GitHub identity, profile and language. |
| M55 | `working_sessions` | Clean | Drop/recreate empty Ask/verify threads; no old runtime session adoption. |

Jobs store payer and PR container identity; partial unique indexes enforce one claimed/running job per
(PR,payer), or per payer for judge_work. Target triggers keep a job's explicit PR equal to its analysis/pass PR.
The two shapes are independent: a payer's judge worker and PR worker may each own a job.
`analytic_threads` is keyed by (PR,user), and last-context generation must equal its analytic generation.
Working sessions are keyed by (PR,user,kind), preserving separate analytic Ask and review Ask threads.
Migration 0030 cleans and rebuilds working_sessions as well. Working preflight is bound to seed, generation and immutable kind; repository acknowledgement additionally
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

W5 uses claudius 13.7.0 (`71096d7d4481977added8761b2cfd63cd84852b4`), including the corrected
`validate_report.py --ids` contract. `PrDataItem.decision` is a standing choice with kind/target/title,
without reply-chain attribution. A missing required target rejects host input before admission.
`SR_MAINTAINER_DELEGATES` explicitly configures comma-separated trusted relay logins (default empty).
Neither bot status nor tenant membership implicitly grants relay authority.

The server archives non-secret inputs under `data_dir/assessment-snapshots/<pass>.json` with mode 0600.
The immutable reference and SHA-256 are stored in the pass; recovery checks both before reading the archive.
The archive contains cleaned source versions, scope, offered IDs, standing decisions, item fingerprints and
the native v4 input. Both method jobs carry identical `pr_data`, head/base and model/effort. The host brief
is `prompts/check_pr_comments.md`; the native v4 prompt and strict schema remain separate.

`admit_review` reserves quota, creates the visible review pass and enqueues its selected method jobs in one
write transaction. Claiming advances the method attempt in that same queue transaction. `finish_with_primary`
accepts normalized rows and primary projections atomically: secondary runs cannot allocate IDs, consume
sources, supersede points, carry decisions or create visible assessments. Failed retryable methods retain
their job, snapshot and admission day; their attempt usage is summed once. A primary failure never promotes
the secondary. Source versions with only `merged_urls` coverage remain unread.
`usage_complete` is false while any started attempt lacks metering. Totals include only known usage; a late
metering record can complete an unknown attempt once without charging its duration again.

`AssessmentMethodRun.reported_head` preserves a report's actual commit. A mismatched head is retained in
comparison rows as `head_mismatch`, excluded from scoring and primary projections. The frozen-head DB
constraint remains intact. Malformed or ungrounded rows are dropped independently; scoped missing body
rows remain explicit in comparisons. `CarryEvidence.quote` preserves grounded restatement provenance.
`CommentAssessment.proposed_decision` also preserves the host's complete target. An external PR URL is
never reduced to a same-repository PR number; an existing follow-up target is never converted into a
new follow-up title. Those targets remain available to W6 through the original proposal and comparison.

The comparison HTTP response contains exact-count `summary` denominators, per-method coverage, missing
fields, failures and proposal-kind/target agreement with actual owner decisions. The method runs carry
usage and duration. Duplicate and mismatched-head rows are not scored; absent verdicts/proposals stay null.
Owner revisions are captured from real diffpilot decisions, including decisions already present when a
primary result is accepted; report `cutoff` selects the latest recorded revision at that instant.
Additional attributed reply/status decisions remain separate `context` comparison rows, with their native
IDs and `recorded_decision` evidence. They allocate no visible items, consume no source and never score as
owner truth. The singular decision on a primary card remains the controlling model-reported evidence.

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

## Private comment Ask without an analysis (approved W8 contract amendment)

CR1 (approved by the coordinator, 2026-11-23): a comment question without a review-ready analysis uses `JobKind::Ask` with
`JobTarget::PullRequestAsk { pull_request_id, repo }` (`type: pull_request_ask`). This target has neither
`AnalysisId` nor `PassId`; unexpected target fields are rejected. `JobSpec.payer` is the authenticated asker.
It uses the (PR, payer) CS1 slot and an immutable `ThreadKind::Verify` working thread, independently of the
analytic and ordinary Ask threads. It never forks an analytic thread and has no MemCan endpoint.

The server supplies a bounded private context pack containing public PR/comment data and only that payer's
latest 20 final answers across the PR. Follow-ups retain their original anchor and owner. Prompt data remains
fenced; answers use the existing Ask schema, wording/size/evidence checks, pinned to the queued checkout.
On restart, pending, answering and provisional private questions reattach to the durable job; submission is
idempotent by question/generation. Working checkpoints retain the existing attempt, seed and generation fences.
A lost working archive falls back to the private pack. W4 must handle this target with the same Verify policy,
full fixed start/fork/resume configuration and `/work` cwd as comment Ask with an analysis.

0030 adds `questions.private_ask` to separate private recovery from legacy assessment-question continuations.
New private questions cannot be selected by those continuations, including the interval before job submission.
`jobs.private_question_id` has a composite FK to (question, PR, user, tenant); CHECKs bind target, payer,
question/stage, kind and absence of assessment attribution. A partial unique index prevents repeated live or
successful jobs for one private question/generation. Queue admission and claim use immediate write transactions;
results and checkpoint publication retain their existing fencing. No assessment pass or daily quota row is
created, including when the payer has already used all 80 daily assessment slots. Ask's own hourly/PR limits
and linked-plan admission still apply. Only unapplied 0030 is amended, with its checksum re-recorded.


### Admission and checkpoint failures

Atomic linked-plan admission returns a distinct missing-connection error, including a link lost between
analysis stages or before Ask submission. Analysis and Ask retain the Settings linking sentence.
Checkpoint metadata disagreement discards only the checkpoint and logs a warning; a current attempt's answer
is still stored. A required first-Ask compaction that is skipped or unconfirmed instead records the non-retryable
`ask_compaction_unconfirmed` failure and discards the answer (ADR-004, D15). A context-pack fallback needs no
compaction. Gateway status refreshes retain the longer usage pause and keep the server's last call and typed
failure unless the gateway reports a newer call. Attempt/lease fencing continues to reject stale results.
The first-Ask threshold is shared
as `contracts::runtime::FIRST_ASK_COMPACTION_TOKENS` (80,000 parent-context tokens).

The judge follows `JobFailureKind::retryable()` for failed jobs: schema/credential/configuration and other
non-retryable codes end the ranking with their fixed cause. This includes `OutputNotJson`: design v3.3 D12
has no special retry exception for it. A successfully completed job whose JSON fails server validation still
uses the existing bounded judge retry path; invalid strict schemas and credential relinking never trigger
model repair. Flow tests pin both strict-schema and relink failures to a single unrepaired turn.
