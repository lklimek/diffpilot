# ADR-026: GitHub App webhooks drive item refresh; no automatic scans; judgements go outdated instead of re-run

- Status: proposed (2026-11-25). Owner decisions P1–P4 and the infrastructure of MemCan TODO `dea64a94`
  (2026-11-24/25); owner answers OQ1–OQ4 of 2026-11-25 in `docs/v1.4-phase-b-plan.md` §11. Plan: `docs/v1.4-phase-b-plan.md`
  §2, §4, §5. Security review SEC-001…013 (Smythe, 2026-11-25,
  `/data/artifacts/diffpilot/2026-11-25/v14-phase-b-sec/sec-findings.md`) adopted.
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
   `pull_request_review_comment`, `pull_request_review_thread`, `issue_comment`, `issues`, `check_suite`,
   `check_run` (failing conclusions only; the rest ignored at receipt), `status` (App permission "Commit statuses:
   read"); `installation*` and `ping` arrive automatically. `workflow_run` is not subscribed (owner OQ2): it
   repeats `check_suite` for GitHub Actions and would need "Actions: read".
2. **Endpoint:** a **separate listener** (`Listener::Webhook`, `SR_WEBHOOK_LISTEN_ADDR`, default port 3002) serves
   only `POST /webhooks/github`; the UI/API listener answers 404 for `/webhooks/*`, and traefik forwards
   `/diffpilot/github` to the webhook port only. Infrastructure requirement: traefik ≥ 3.6.4 (or 2.11.32), which fixes
   the 2025 matcher bypasses (CVE-2025-47952, CVE-2025-66490). Own auth kind (`GithubSignature`): headers and
   `Content-Length` checked before the body is read, body ≤ 4 MiB (route-level limit), load shedding at 8 concurrent
   requests with an 8 s timeout, exactly one `X-Hub-Signature-256` (`sha256=` + 64 hex), HMAC-SHA256 over the raw
   body in constant time **before** parsing, then the hook target must be this App; 401 without detail on failure.
3. **Trust:** **the WAF and source IPs are not authentication** (GitHub's hook ranges deliver any user's webhooks to
   any URL). The App `claudbot` is public, so a valid signature proves only "GitHub, our App"; deliveries are
   processed only from installations of the accounts in `SR_GITHUB_WEBHOOK_ACCOUNTS` (resolved to numeric
   installation and account ids; fail closed when unset). Others are recorded `not_allowed` and do nothing.
4. **Delivery handling:** dedupe by `X-GitHub-Delivery` in `github_deliveries`, and by body SHA-256 for 14 days (the
   GUID is not signed, so a captured body replayed under new GUIDs is a duplicate); a failed processing stays
   retryable (GitHub redelivery reuses the id). The handler stores only typed change keys (repo, number, kind, head
   SHA, action), answers 202 and an in-process worker processes them; rows kept 14 days.
5. **Events are signals, not state:** processing re-reads GitHub with the affected user's token through the
   targeted re-read (ADR-022 §5, extended to issues), in a fixed 10 s window per user that a stream cannot extend,
   within a per-user hourly budget of event-driven reads, so order and duplicates are harmless. Check and status
   events match by (repository, head SHA). Watched PRs in live repositories are probed only on
   events (at most once a minute; no periodic probe, owner OQ3); watched PRs in repositories without the App are
   probed once an hour. A new item's
   relation comes from the re-read, never from the event.
6. **Facts only:** an event never re-judges an existing judgement. A judgement whose inputs (facts) or prompt changed
   is *outdated*; the owner re-judges one item by a click or all by "Refresh and re-judge outdated". Never-judged
   items are judged automatically within the daily cap (event-added items at most 20 per user and day; no job at
   zero room). The nightly scan judges only never-judged items.
7. **No automatic refresh:** `dashboard::fresh` (F2, F3) is removed; manual Refresh and the nightly scheduled scan
   (ADR-020) stay. Repositories without the App (or a server without the webhook secret) are refreshed only by those
   and are marked "not live".
8. **Missed events:** our own processing failures retry (3×, then `failed`, resumed at start-up). Deliveries GitHub
   could not hand over (server, tunnel or traefik down) are **not** fetched or redelivered automatically (owner OQ1,
   2026-11-25): the nightly scan and manual Refresh recover the facts; the owner may redeliver by hand in the App
   settings.
9. **Configuration:** `SR_GITHUB_WEBHOOK_SECRET` (secrets.env), optional `SR_GITHUB_WEBHOOK_SECRET_PREVIOUS` for
   rotation, `SR_GITHUB_WEBHOOK_ACCOUNTS`, `SR_WEBHOOK_LISTEN_ADDR`; content type `application/json`. Without the
   secret the server starts, logs once, answers 503 on the route and marks nothing live.
   `SR_GITHUB_WEBHOOK_URL` is optional and only shown in Settings. Logs carry no body, signature or payload text;
   traefik and cloudflared must not log request bodies.
10. **Security invariants of what events can cause:** an event triggers reads only; it never re-judges an existing
   judgement and never causes a GitHub write. A decision imported from GitHub (ADR-023 amendment 2026-11-25) never
   has effects; only a later decision of the owner in diffpilot acts.

## Alternatives considered
| Alternative | Why not |
|---|---|
| Keep F2/F3 polling | the owner rejected periodic scans; costs ≈600–1,000 GitHub points per open hour |
| GitHub notifications API | classic PAT only (no App user tokens), CI not covered (owner P1) |
| Search polling for changes | a scan by another name; same cost and latency trade-off |
| Trust the payload as the new facts | payload shapes differ per event and lack derived facts (rollups, stacks); a re-read keeps one fact path and makes order irrelevant |
| Store raw payloads | needless retention of PR texts; the change keys are enough |
| Webhook route on the UI/API listener | one traefik rewrite would be the only barrier to the UI and API; matcher bypasses happened in 2025 |
| Trust any installation of the public App | outsiders' installations deliver validly signed events and could add repositories or force reads |
| A webhook relay service (smee, hookdeck) | an extra third party for a path the owner's infrastructure already provides |
| All `check_run` events | one event per CI job; only failing conclusions add a fact before the suite completes |
| `workflow_run` | repeats `check_suite` for Actions and needs "Actions: read" (owner OQ2) |
| Redelivery sweep of failed deliveries (App JWT, every 30 min) | owner OQ1: the nightly scan and Refresh are enough; one moving part less |
| Periodic watch probe in live repositories | owner OQ3: events are enough; the hourly probe stays only where no events arrive |
| Re-judge changed items automatically within the caps | the cap was spent on small changes; the owner wants explicit re-judging (P3) |

## Consequences
- Contract changes: `RouteAuth::GithubSignature`, `Listener::Webhook`, route `githubWebhook`, C1–C3 and C13 of the
  phase B plan; migration `0031_github_webhooks.sql`; App settings change (webhook on, events, "Commit statuses:
  read" accepted by each installation); a third published port that only traefik reaches; `docs/deployment.md`,
  runbook (traefik version, rotation order) and threat model (T30) are updated.
- Facts of live repositories are minutes fresh without an open page; non-live ones are as fresh as the last
  manual or nightly scan, and the UI says so.
- Rankings show more *outdated* judgements; the owner's clicks, not a timer, spend the daily cap.
- A public inbound endpoint exists; its safety rests on the separate listener, the HMAC secret, the installation
  allow-list and the fact that events only trigger bounded reads. The WAF only reduces noise. Residual risk: a party
  holding a captured signed body can replay it after the 14-day dedupe window (cost only, T30).
