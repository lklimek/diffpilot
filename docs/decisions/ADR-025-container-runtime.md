# ADR-025: Per-payer containers and gateway boundary

- Status: accepted, implemented container runtime; KL1-A selects ChatGPT-plan linking only.
- Source: [design v3.3](http://agentic/diffpilot/2026-11-23/v13-containers/design-v3.md),
  S14-1/2/3/4/5, 2026-11-23; integrated claudius 13.7.0 source pin `71096d7` (W5-authorized update from 13.5.0 `460e5fd`).

Every PR job uses a persistent container per (PR,payer), with one turn at a time. JudgeWork uses a separate
answer-only user container. Long-lived app-server replaces exec; the launcher alone owns the Docker socket.
Workers hold only expiring virtual model keys and per-job read-only GitHub installation tokens in tmpfs files.
Real provider credentials and their durable lifecycle belong only to a separate gateway service/store.

The container is the security boundary (SG1–SG16). No host Codex home is mounted. Workers use Squid with LAN
denied except configured MemCan; gateway upstreams are fixed. Hosted web/image tools are refused by gateway
body checks. Repository code is never built, tested or run. CS1 cwd is `/work`, with `/work` and `/work/repo`
untrusted in system and rewritten user config. Critical runtime overrides and full immutable thread configs
apply before every start/fork/resume. CS2 runner setup is filesystem-only and judge model tools fail the job.

Analytic sessions belong to (PR,payer); the volume is their archive. Same-payer Ask forks with fixed read-only
MemCan config; other users and volume-loss recovery use a private context pack. The first-Ask compaction
preflight above 80k is explicit and durable. Usage sums turn deltas, including compaction, excluding inherited
fork usage. Missing or stale context is recovered before deciding whether to compact.

Gateway refresh uses a dedicated chain with one owner, near-expiry refresh only, durable in-flight markers
and persist-before-use. Unknown outcome quarantines the generation and requires relink; it is never replayed.
Only explicit unlink permits upstream revoke. Tests use fake auth endpoints/tokens; no real-account probes.
KL1-A selects gateway-run device linking with singleton `mode: chatgpt_plan`. Link requests reject secrets,
API keys and base URLs; migration 0030 constrains mode and state. Linking may log Codex out elsewhere;
Settings must disclose this risk. No paid fallback or secret export is authorized.

Assessments retain native prompt v4 beside claudius host mode. Per-user settings freeze the primary and optional
A/B pair at admission. Both methods share immutable input, head/base and model, but have independent job/attempt
fences; only the primary changes visible items and effects. The secondary stores comparison data only. One
logical pass, including a pair, consumes one of 80 UTC daily slots atomically with all method jobs; retries keep
the same pass/day. Actual owner decision revisions are append-only comparison truth.

The owner's 2026-11-23 cut-over decision supersedes the empty start, guarded 0030 and D18/W10 settings
export/import. Migration 0030 upgrades the existing database in place, atomically cleaning analysis/runtime
data while preserving users, sign-in/GitHub auth, tenants/allow-list, installation and dashboard settings,
channels, PAT/webhook configuration, watches and snoozes. The complete keep/clean inventory is in the
[container contracts](../contracts/containers.md#persistence-and-cut-over). No stored auth is read or logged.
Old Codex credentials are dropped/recreated empty and plan-only (CCR10); gateway relinking is required.
A fresh database remains supported. Applied migrations remain append-only; 0030 is still unmerged.
Database code and transactions remain behind the db repository API.

## v1.5 payer preferences

A model and reasoning effort, Ask effort and dashboard ranking effort belong to the user whose plan
pays for a job. Migration 0039 adds personal overrides and retains the installation choice as a
read-only fallback. Built-in defaults apply where neither value exists. Settings writes only the
authenticated user's requested field, across all their tenant memberships.

Admission snapshots the current effective choice into the queued job. New analyses use the creator,
explicit analysis retries use the retrying user, Ask uses the asker, and comment assessment uses its
initiator or selected watcher. Ranking uses the dashboard user. Both jobs of a paired assessment use
the same payer and selection. A lease or method retry retains its existing job spec.

This supersedes ADR-013 and the unmerged ADR-024 design. It amends ADR-002/003/004/005/017/023 and plan §5.4.
`runner::provider` and `server::gateway` alone know Codex; gateway may use secrets/config/contracts but not db.
Runner gains launcher mode and gateway admin access, with no DB or GitHub API client. JobSpec remains
self-contained, model selection and fake provider remain, and server/worker checker format v3 is rebuilt together.

See [container contracts](../contracts/containers.md) for types and persistence, the
[threat model](../contracts/threat-model.md) for SG1–SG16 and accepted risks, and the
[runbook](../runbook.md) for fresh start, in-place 0030 upgrade, backup and Settings linking.

Deployment has five images: server (including gateway binary), runner/launcher, worker, fetch and Squid.
Only launcher holds the Docker socket, with no network. Separate internal worker and judge networks expose
only model listeners; admin is bound to the runner interface. Worker/fetch references are digest-pinned.
A registry with its host port published on 127.0.0.1 supplies real digests for local builds. New instances use
a dedicated NAT bridge with inter-container communication disabled and a digest-pinned image; running legacy
instances are preserved with a notice until owner-scheduled replacement. `deploy/dev/local-containers.sh` prepares images,
new runtime keys and non-secret configuration; it never deploys the live application stack. The credentials
scope file may be `10010:<host-gid> 0440` locally so the host-uid server can read it; other gateway secrets
stay `10010:10010 0400`, and the runner's keys copy is `10002:10002 0400`.

Validation: offline image tests, CS3 through Squid, live SG5/SG7 probes with fake listeners, MSW Playwright and
full-stack fake-provider journeys. Security review is a separate coordinator gate. No real-account auth probe
is part of acceptance. The worker/launcher/gateway are trusted infrastructure; Docker operators remain trusted.
