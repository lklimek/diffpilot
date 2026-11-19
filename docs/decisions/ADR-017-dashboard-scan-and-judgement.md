# ADR-017: Dashboard scan on the server with the user token; LLM judgement as a runner job

- Status: accepted (owner answers 2026-11-18: OD3, OD4, OD6, OD7, OD10–OD13, OD16, OD18, OD19);
  plan `docs/v1.1-dashboard-plan.md` §A, §B, §C, §G. Depends on ADR-018 for the job shape.
- Spikes: S11-2 done (`docs/spikes/S11-2-scan-cost.md`), S11-4 done (`docs/spikes/S11-4-memcan-todos.md`),
  S11-3 (judgement quality) open.

## Context
The `daily-priorities` skill scans GitHub with `gh` (the owner's credential) across many repositories and
organisations, computes mechanical score parts, lets the model judge the rest, and merges both into a ranking.
In smart-review the runner must not hold user tokens (T3, ADR-005) or contain GitHub client code, and only
`runner::provider` may talk to Codex (A5). An installation token covers one installation and cannot express
"everything this user works on".

## Decision
1. **Scan on the server.** A Rust port of `scan.py`, `stats.py` (pull requests only, OD16) and `todos.py` runs as
   a server task started by `POST /api/v1/dashboard/refreshes` or by the daily schedule (ADR-020). GitHub calls are in
   `server::github::work`, read-only (REST `GET`, GraphQL queries), with the signed-in user's GitHub App user
   token (refresh serialised per user, ADR-007). REST search for discovery with the skill's pacing (3 s, 60/120 s retries); GraphQL
   details batched by alias, 10 PRs per request, never more than 20 (new work: the skill sends one query per PR,
   S11-2); blocker relations from one open-PR ref listing per repository instead of per-PR `gh pr list`; enumeration of configured owners for logins search refuses. Inventory: plan §A.2.
2. **Reach is assumed, then reported.** The owner installs or grants the App where he works (OD7); each refresh
   looks up the configured repositories and owners with the user token and reports the invisible ones as scan
   notes and in Settings.
3. **Deterministic scoring** in `server::dashboard::score` (pure): the skill's components, formulas, CI
   classification, eligibility, tiers and sort, driven by per-user settings with code-constant fallbacks (plan
   §C.2), and judgement reuse by the judged-input fingerprint and prompt version (plan §B.4). Not-yet-judged items show computed parts plus the
   skill's conventional-commit estimates (approved code constants), named as estimates in the basis text.
4. **Judgement is a runner job** of kind `judge_work`, submitted through `JobExecutor` in batches of up to 8
   items. The `JobSpec` embeds the items' scan facts in its prompt (self-contained, A4); no checkout, GitHub token,
   MemCan endpoint, session or skills; priority interactive. `select_model(JobKind::JudgeWork, …)` returns the
   installation model with the installation-wide dashboard ranking effort (default low, set in Settings next to
   the model/effort choice; other kinds keep the saved effort). The server validates output against closed value
   sets and stores judgements per (user, item). While an analysis job holds the credential the ranking waits and
   the page says "Waiting for the review analysis to finish".
5. **The LLM judges only what the skill leaves to judgement** (plan §B.1), and **never twice for the same judged
   inputs** (plan §B.4): unchanged items reuse their stored judgement, no job is created when nothing changed, and
   per-refresh and per-day caps bound the rest. A prompt or schema change takes effect only through an explicit
   prompt-version bump. A judgement sets only the parts the scorer asks about, including the computed defaults
   sent for confirmation (status `estimated`); an answer for a part computed from facts is dropped, the rest of
   the answer counts (owner decision on AR-9, 2026-11-18).
6. **MemCan TODOs** are read by the server's MemCan client with `list_todos` per MemCan project (limit 100,
   open statuses filtered client-side, per-status calls when `has_more`) for each user with "show my MemCan TODOs"
   on (default on); no writes. MemCan has no users or project listing, so the projects are a per-user setting:
   the short names of the user's dashboard repositories plus extra names (S11-4). MemCan failure keeps the previous TODO rows with a note.
7. **History for charts** (PRs opened/merged/closed, reviews given) is computed from GitHub at each refresh over a
   12-week window and stored as dated events in the snapshot; no accumulated history tables.
8. **Private repositories** the token reads are included and shown only to that user.

## Alternatives considered
| Alternative | Why not |
|---|---|
| Scan in the runner with the user token | breaks T3 and ADR-005; GitHub client code in the runner |
| Scan by an agent through `github-mcp-server` with an installation token | one installation cannot search the user's work across orgs; slow, costly, non-deterministic |
| Server calls Codex directly | only `runner::provider` knows Codex (A5); the server has no Codex or credential |
| Python skill as a sidecar | second runtime, untyped output, no contract generation |
| GraphQL `search` for discovery and details in one call | limits for user tokens unverified; a later optimisation at most |
| MemCan `export_collection` over all projects | raw SQL filter built from input, unordered, double payload (S11-4) |
| Accumulated daily snapshots for charts | GitHub search gives the history at refresh time |
| Judgement effort tied to the review effort | the owner wants a light default for ranking and the review effort unchanged |

## Consequences
- New `JobKind` and a non-PR job target (ADR-018); `llm_selection` gains `judge_effort`.
- Ranking shares the single Codex credential (ADR-003) and waits behind analysis jobs.
- Private-repository titles and excerpts are stored in snapshots and sent to the provider in judge prompts
  (threat-model T16–T19).
- Rate limits are per user token; a refresh fails with a retry time instead of exhausting the limit.

## Addendum (2026-11-18): excluded authors and known bots
Owner decision. `skip_authors` ("excluded authors") is a list of banned authors: every pull request and issue a
listed login authored is left out of the dashboard (Needs you and Waiting on others), also when a review is
requested from the user or it is assigned to the user, so it never reaches the judge; the user's own reviews on
such items still count in the history. Being listed does not make a login a bot. A bot is GitHub's `Bot` type, a `[bot]` login or a login in the per-user `known_bots`
(migration 0009; default `thepastaclaw`, `Claudius-Maginificent`, `DCG-Claude`, the former built-in list); nothing
else is hard-coded. Bot classification reaches the judge only through the scan facts and scorer signals, so editing
either list re-judges just the items whose judged inputs change.
