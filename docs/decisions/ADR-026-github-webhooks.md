# ADR-026: GitHub App webhooks drive item refresh; no automatic scans; judgements go outdated instead of re-run

- Status: proposed (2026-11-25). Owner decisions P1–P4 and the infrastructure of MemCan TODO `dea64a94`
  (2026-11-24/25); open points in `docs/v1.4-phase-b-plan.md` §11 (OQ1–OQ4). Plan: `docs/v1.4-phase-b-plan.md`
  §2, §4, §5.
- Supersedes plan `docs/v1.1-dashboard-plan.md` §M (F2 automatic scans, F3 fact checks; v1.3 MW1). Reverses the
  rejected alternative "GitHub webhooks for watched PRs" of ADR-023 (amendment 2026-11-25 there). Extends ADR-020
  (nightly scan rule), ADR-022 §5 (targeted re-read is the event path) and ADR-017 (judgement reuse rule).

## Context
v1.3 kept My work fresh by polling while it was open: a full scan every 30 min (15 on open) and a fact probe every
5 min (plan §M). The owner did not want periodic scans: when something tracked changes on GitHub, only that item
should refresh; a full scan runs only at night. The server is on the LAN; since 2026-11-25 a public path exists:
GitHub → Cloudflare WAF (GitHub `hooks` ranges only) → atlas tunnel → traefik → `POST /webhooks/github`
(`https://webhooks.manago.cloud/diffpilot/github`, Host rewritten, client IP only in `Cf-Connecting-IP`).
Automatic re-judging of changed items spent the daily cap (120) on items whose ranking barely moved.

## Decision
1. **Event source:** the GitHub App's webhook. Subscribed: `pull_request`, `pull_request_review`,
   `pull_request_review_comment`, `pull_request_review_thread`, `issue_comment`, `issues`, `check_suite`, and
   `status` if the owner adds "Commit statuses: read" (OQ2); `installation*` arrive automatically.
2. **Endpoint:** public route outside `/api/` with its own auth kind (`GithubSignature`): body ≤ 4 MiB, required
   GitHub headers, HMAC-SHA256 (`X-Hub-Signature-256`, constant time) over the raw body **before** parsing; 401
   without detail on failure. Source IPs are never authentication.
3. **Delivery handling:** dedupe by `X-GitHub-Delivery` in `github_deliveries`; a failed processing stays
   retryable (GitHub redelivery reuses the id). The handler stores only typed change keys (repo, number, kind, head
   SHA, action), answers 202 and an in-process worker processes them; rows kept 14 days.
4. **Events are signals, not state:** processing re-reads GitHub with the affected user's token through the
   targeted re-read (ADR-022 §5, extended to issues), coalesced 10 s per user, so order and duplicates are harmless.
   Watched PRs get the watch probe at once.
5. **Facts only:** an event never re-judges an existing judgement. A judgement whose inputs (facts) or prompt changed
   is *outdated*; the owner re-judges one item by a click or all by "Refresh and re-judge outdated". Never-judged
   items are judged automatically, whatever brought them in. The nightly scan judges only never-judged items.
6. **No automatic refresh:** `dashboard::fresh` (F2, F3) is removed; manual Refresh and the nightly scheduled scan
   (ADR-020) stay. Repositories without the App (or a server without the webhook secret) are refreshed only by those
   and are marked "not live".
7. **Missed events:** our failures retry (3×, then `failed`, resumed at start-up); GitHub's failed deliveries are
   listed with the App JWT (`GET /app/hook/deliveries?status=failure`) and redelivered at start-up and every
   30 min (OQ1); GitHub keeps them 3 days; beyond that the nightly scan is the backstop.
8. **Configuration:** `SR_GITHUB_WEBHOOK_SECRET` (secrets.env) and `SR_GITHUB_WEBHOOK_URL` (display in Settings),
   both or neither. The App's webhook is turned on with that URL and secret.

## Alternatives considered
| Alternative | Why not |
|---|---|
| Keep F2/F3 polling | the owner rejected periodic scans; costs ≈600–1,000 GitHub points per open hour |
| GitHub notifications API | classic PAT only (no App user tokens), CI not covered (owner P1) |
| Search polling for changes | a scan by another name; same cost and latency trade-off |
| Trust the payload as the new facts | payload shapes differ per event and lack derived facts (rollups, stacks); a re-read keeps one fact path and makes order irrelevant |
| Store raw payloads | needless retention of PR texts; the change keys are enough |
| A webhook relay service (smee, hookdeck) | an extra third party for a path the owner's infrastructure already provides |
| `check_run` events | one event per CI job; `check_suite completed` carries the same rollup change |
| Re-judge changed items automatically within the caps | the cap was spent on small changes; the owner wants explicit re-judging (P3) |

## Consequences
- Contract changes: `RouteAuth::GithubSignature`, route `githubWebhook`, C1–C3 and C13 of the phase B plan;
  migration `0031_github_webhooks.sql`; App settings change (webhook on, events, possibly a new permission each
  installation accepts); `docs/deployment.md`, runbook and threat model (T30) are updated.
- Facts of live repositories are minutes fresh without an open page; non-live ones are as fresh as the last
  manual or nightly scan, and the UI says so.
- Rankings show more *outdated* judgements; the owner's clicks, not a timer, spend the daily cap.
- A public inbound endpoint exists; its safety rests on the WAF, the HMAC secret and the fact that events only
  trigger reads.
