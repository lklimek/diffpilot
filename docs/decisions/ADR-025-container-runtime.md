# ADR-025: Per-payer containers and gateway boundary

- Status: accepted common contracts; credential-mode selection KL1 remains open.
- Source: [design v3.2](http://agentic/diffpilot/2026-11-23/v13-containers/design-v3.md),
  S14-1/2/4/5, 2026-11-23. S14-3 remains a W5 acceptance gate.

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
KL1 has not selected plan, API-key or both: common DTOs and migration contain no new mode discriminator,
API-key input or mode-specific constraints, and authorize no implicit paid fallback or secret export.

The v1.3 cut-over starts empty. Guarded migration 0030 refuses non-empty rebuilt tables atomically; settings
carry-over is W10 and excludes secrets. Database code and transactions remain behind the db repository API.

This supersedes ADR-013 and the unmerged ADR-024 design. It amends ADR-002/003/004/005/017/023 and plan §5.4.
`runner::provider` and `server::gateway` alone know Codex; gateway may use secrets/config/contracts but not db.
Runner gains launcher mode and gateway admin access, with no DB or GitHub API client. JobSpec remains
self-contained, model selection and fake provider remain, and server/worker checker format v3 is rebuilt together.

See [container contracts](../contracts/containers.md) for W1 types and persistence. W3/W4/W5/W8 enforce the
runtime/ingest behavior; W9 removes the old runtime, and W7 completes deployment/security/runbook acceptance.
