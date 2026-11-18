# ADR-017: Dashboard scan on the server with the user token; LLM judgement as a runner job

- Status: accepted (owner answers 2026-11-18: OD3, OD4, OD6, OD7, OD10–OD13, OD16, OD18, OD19);
  plan `docs/v1.1-dashboard-plan.md` §A, §B, §C, §G. Depends on ADR-018 for the job shape.
- Spikes: S11-2 (scan cost and pacing), S11-3 (judgement quality), S11-4 (MemCan export shape).

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
   token (refresh serialised per user, ADR-007). REST search for discovery with the skill's pacing; GraphQL
   details batched by alias; enumeration of configured owners for logins search refuses. Inventory: plan §A.2.
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
   prompt-version bump.
6. **MemCan TODOs** are read by the server's MemCan client (`export_collection`, paged) for each user with
   "show my MemCan TODOs" on (default on); no writes. MemCan failure keeps the previous TODO rows with a note.
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
| GraphQL `search` for discovery and details in one call | limits for user tokens unverified; S11-2 may adopt it later as an optimisation |
| Accumulated daily snapshots for charts | GitHub search gives the history at refresh time |
| Judgement effort tied to the review effort | the owner wants a light default for ranking and the review effort unchanged |

## Consequences
- New `JobKind` and a non-PR job target (ADR-018); `llm_selection` gains `judge_effort`.
- Ranking shares the single Codex credential (ADR-003) and waits behind analysis jobs.
- Private-repository titles and excerpts are stored in snapshots and sent to the provider in judge prompts
  (threat-model T16–T19).
- Rate limits are per user token; a refresh fails with a retry time instead of exhausting the limit.
