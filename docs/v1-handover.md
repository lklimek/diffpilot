# diffpilot: handover after v1 and v1.1

Status of the code on `feat/v1`. Authoritative design: `docs/design-brief.md` (with its amendments),
`docs/implementation-plan.md`, `docs/v1.1-dashboard-plan.md`, `docs/decisions/` (ADR-001 to ADR-020). Operating the
stack: `docs/runbook.md`. Build and test commands: `CLAUDE.md` (not repeated here).

## What exists

Review module (v1, `/review`, `/analyses/{id}`):
- PR URL, check, explicit start, then plan, PR context and flow analysis by Codex; the Tour shows the final code with
  changed regions grouped into flows, step by step, with LLM text separated from deterministic facts.
- Flags (red or yellow, with a scenario and a not-checked line) replace "questions to check" in flow analysis
  (brief §1 addendum, ADR-014 addendum, flow file format version 2). Older analyses show their questions as flags of
  unknown level.
- Ask is a conversation per user and analysis: one thread across steps, its own effort setting (default medium,
  Settings), a faster checkout for the Ask job, a resizable Ask area, "ask about this line" on every code line.
- "Use as review comment" turns a flag or an Ask answer into an editable draft in the comment composer; nothing is
  posted until the user adds it. Pending review comments only; the tool never submits a review.
- Browser tab title shows the analysis status; a checks link opens the PR's checks on GitHub.

Dashboard module (v1.1, `/`):
- "My work": the user's PRs, issues and optional MemCan TODOs, ranked P0 to P3 by a computed score with visible
  components, and pull-request throughput charts. Read-only towards GitHub and MemCan (ADR-016, ADR-017).
- LLM ranking by `judge_work` jobs, which need no checkout or session; judgements are reused for unchanged items; caps
  of 48 items per refresh and 120 per user and day (ADR-017, ADR-018).
- Daily scheduled refresh, default 06:00 in the user's zone, with catch-up and `needs_sign_in`; the server keeps the
  encrypted GitHub token pair up to 30 days after the last sign-in (ADR-020).
- Personal access tokens (`srp_...`, read-only) for agents; routes in `docs/contracts/agent-api.md` (ADR-019).
- Settings gained Dashboard, Access tokens and the dashboard ranking effort; Ask effort sits beside the model choice.

## Where things live

| Path | Content |
|---|---|
| `crates/contracts` | wire types, no IO; routes (`src/routes.rs`); generated schemas in `schemas/` and TS types |
| `crates/analysis` | diff, lines, coverage, moves, tagged flow format (`tagged`), tech plugins (`tech`) |
| `crates/db` | sqlx, migrations 0001 to 0007, repositories (the only code using sqlx) |
| `crates/runner` | job execution, `provider` (the only Codex code), supervisor, `fake-provider` feature |
| `crates/server` | HTTP API, auth, jobs and claim, pipeline, review, Ask, `dashboard/` (scan, score, judge, refresh, schedule), `github/work`, `memcan/todos` |
| `web/` | React UI: `src/components` (self-contained blocks), `src/screens`, msw mocks, vitest, Playwright (msw) |
| `e2e/` | full-stack Playwright specs against server, runner with the fake provider and the fake tools (`e2e/README.md`) |
| `tools/fake-github`, `tools/fake-memcan` | fake services for tests |
| `deploy/` | compose stack, Dockerfiles, `backup.sh`, `check-production-config.sh`, `test-stack.sh`, `dev/` |
| `scripts/` | architecture, migration and wording checks, schema smoke, `tests/` |
| `docs/contracts`, `docs/decisions`, `docs/spikes`, `docs/mockups` | contracts, ADRs, spike results, mockups |

## Build and test

See `CLAUDE.md` "Build and test". One heavy job at a time on the owner's host.

## Known limits

1. One analytic session per pull request and one private session per (PR, user) for Ask; `generic` tech plugin has no
   call hierarchy (`CLAUDE.md`).
2. Dashboard data cannot be wiped from the UI or API; old refreshes and judgements are pruned automatically
   (`docs/runbook.md`, "Operating the dashboard").
3. Analyses stored before flags are not converted (`docs/decisions/ADR-014-flow-file-output.md` addendum).
4. The runner's scratch areas have no size bound; it only refuses jobs below a free-disk reserve
   (`docs/contracts/threat-model.md:132`, `docs/decisions/ADR-013-runner-process-model.md:126`).
5. The code view does not mark bidi or invisible characters (`docs/contracts/threat-model.md:129`, `:174`).
6. Cleanup of archived agent sessions after a user is removed from the allow-list is deferred
   (`docs/contracts/threat-model.md:49`).
7. The Ask fork limit of 80000 tokens is not measured on the deployed Codex
   (`docs/decisions/ADR-004-agent-sessions.md:97`).
8. Parallel flow analysis and a long-lived Codex app-server are deferred (`docs/decisions/ADR-015-parallel-flow-analysis-deferred.md`).

## Open follow-ups

`TODO(` markers in code:

1. `web/src/screens/dashboard/DashboardScreen.tsx:209`: the contract has no review-request age per item, so "waiting
   on you" stays unknown.
2. `web/src/screens/settings/SettingsScreen.tsx:43`: remove the dark-launch hide of the dashboard blocks (the
   dashboard routes now ship).
3. `e2e/tests/settings.spec.ts:76`: the fake GitHub always answers the installations list, so the "not checked"
   state is untested.
4. `e2e/tests/comments.spec.ts:156`: `comment_published` needs a review submitted between the server's check and its
   write; the fake cannot do that.

Open items in docs:

1. `docs/design-brief.md:517`: static analysis (tree-sitter tags) in the language plugins.
2. `docs/design-brief.md:603` to `:616`, "Open points": GraphQL pending-review behaviour, Codex's own sandbox in an
   unprivileged container, neutralising repository-provided agent configuration, Portainer and containers created
   outside the stack, Codex resume and compaction in non-interactive mode, file-level pending comments,
   `github-mcp-server` with an installation token.
3. `docs/decisions/ADR-013-runner-process-model.md:126`: a soft per-job disk quota.
4. `docs/contracts/threat-model.md:239` (AR-9): LLM score parts can be skewed by item text within closed value sets;
   per-part caps and gap marking are proposed.
