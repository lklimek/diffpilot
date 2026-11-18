# ADR-017: Dashboard scan on the server with the user token; LLM judgement as a runner job

- Status: proposed (architect, 2026-11-18), on owner decisions OD3, OD4, OD6 of 2026-11-18;
  plan `docs/v1.1-dashboard-plan.md` §A, §B, §C, §G. Depends on ADR-018 for the job shape.
- Open items answered by spikes S11-1 (token reach), S11-2 (cost), S11-3 (judgement quality), S11-4 (MemCan).

## Context
The `daily-priorities` skill scans GitHub with `gh` (the owner's credential) across many repositories and
organisations, computes mechanical score parts, lets the model judge the rest, and merges both into a ranking.
In smart-review the runner must not hold user tokens (T3, ADR-005) or contain GitHub client code, and only
`runner::provider` may talk to Codex (A5). The installation token is per installation and repository and cannot
express "everything this user works on".

## Decision
1. **Scan on the server.** A Rust port of `scan.py`, `stats.py` and `todos.py` runs as a server task started by
   `POST /api/v1/dashboard/refresh` (manual only, OD3). GitHub calls are in `server::github::work`, read-only
   (REST `GET`, GraphQL queries; never mutations), with the signed-in user's GitHub App user token from
   `auth::tokens` (refresh serialised per user, ADR-007). Discovery uses REST search (as `gh search` does) with
   the skill's pacing (≥3 s between searches, retries 60 s/120 s on the secondary limit); details use the skill's
   GraphQL selection batched by alias; the unsearchable-login fallback enumerates the scope's owners by GraphQL.
   The full query inventory is plan §A.2.
2. **Scoring is deterministic Rust** in `server::dashboard::score` (pure, no IO): the skill's components,
   formulas, CI classification, eligibility, tiers and sort, with judgement inheritance by item fingerprint.
3. **Judgement is a runner job** of the new kind `judge_work`, submitted through `JobExecutor` in batches of up
   to 8 items. The `JobSpec` carries the scan facts of those items in its prompt (self-contained, A4); the job
   has no checkout, no GitHub token, no MemCan endpoint and no session (one-shot; nothing archived); priority
   interactive. `select_model(JobKind::JudgeWork, …)` returns the installation model at low effort (S11-3 may
   set medium). The server validates the output against closed value sets and stores judgements per (user, item).
4. **The LLM judges only what the skill leaves to judgement** (plan §B.1: defect likelihood/impact, change impact,
   completion, urgency, conditional blocked/unblocks/waiting/advancement/milestone, eligibility flips, dedup among
   server suggestions, P0 alarm, why/next text). Everything else is computed. Unjudged new items stay "Not judged
   yet" (no heuristic estimates); changed items keep their previous judgement as provisional.
5. **MemCan TODOs** are read by the server's MemCan client (`export_collection`, paged) for users who opted in;
   no writes (OD4). MemCan failure keeps the previous TODO rows with a note.
6. **History for charts** is computed from GitHub at each refresh over a 12-week window and stored as dated events
   in the snapshot; no accumulated history tables.

## Alternatives considered
| Alternative | Why not |
|---|---|
| Scan in the runner with the user token | breaks T3 and ADR-005; puts GitHub client code in the runner |
| Scan by an agent through `github-mcp-server` with an installation token | the token covers one installation, cannot search the user's work across orgs; slow, costly, non-deterministic |
| Server calls Codex directly for judgement | only `runner::provider` knows Codex (A5); the server image has no Codex and no credential |
| Python skill as a sidecar | second runtime and image, untyped output, no contract generation |
| GraphQL `search` for discovery + details in one call | fewer calls, but its limits for user tokens are unverified; S11-2 may adopt it later as an optimisation |
| Heuristic estimates for unjudged items (`score.py --heuristics`) | OD3 wants new items under "Not judged yet"; heuristics would look like judgements |
| Accumulate daily snapshots for charts | GitHub search gives the history at refresh time; tables and backfill logic for nothing |

## Consequences
- New `JobKind` and a non-PR job target (ADR-018).
- The ranking shares the single Codex credential (ADR-003): while an analysis job runs, ranking waits (shown as
  queued); deterministic scores are visible meanwhile.
- Private-repository titles and excerpts the user token can read are stored in snapshots and sent to the provider
  in judge prompts; threat-model entries T16–T19 and the owner question Q11-3.
- Rate limits are per user token; the refresh fails with a retry time instead of exhausting the limit.
- Unverified until S11-1: search reach of App user tokens outside the App's installations; whether
  `Claudius-Maginificent` stays unsearchable with a user token.
