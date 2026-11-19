# ADR-016: GitHub work dashboard as the start page; review module boundary; trust-rule boundary

- Status: accepted (owner, 2026-11-18; brief amendment approved), on owner decisions OD1, OD2, OD5, OD15, OD16, OD18;
  plan `docs/v1.1-dashboard-plan.md` §0, §F.
- Supersedes: brief §2 v1 "Input … No dashboard, no recent-reviews list" (dashboard part) and §2 Later
  "v3+: LLM-curated dashboard of PRs to review" (brief amendment below, inserted by WP11-DOCS).

## Context
The owner replaces the `daily-priorities` skill with a page in diffpilot: the PRs and issues the user authored,
is assigned or is asked to review, ranked with computed numeric scores and charts. The existing product (PR URL →
check → Start analysis → Tour) stays and becomes reachable from that page. Brief §1 forbids numbers that read as
confidence and badges for non-deterministic content; a numeric ranking would break those rules if they applied
to every screen.

## Decision
1. **Start page.** `/` is "My work" (dashboard). The review start page moves to `/review` (the v1 Home screen,
   unchanged except: arriving with `?pr=` resolves the PR without starting an analysis and shows new / analysis
   found / PR has moved). `/` with a `pr` or `login_error` query redirects to `/review` with the same query, so
   existing deep links (`/?pr=` from the Tour, the OAuth error redirect) keep working without a server change.
2. **Module boundary.** The dashboard is a separate module: server `server::dashboard` (+ `server::github::work`,
   `server::memcan::todos`), DB `db::repo::dashboard`, contracts `contracts::api::dashboard` and
   `contracts::agent::judge_work`, web `components/dashboard/**`, `screens/dashboard/**`. It reads the review
   module only through the database (existing analyses of a PR, head moved, `has_my_state`) and links to it by
   URL; the review module does not know the dashboard. A PR row links to `/analyses/{id}` when an analysis of the
   current head exists, otherwise to `/review?pr=<url>`; a PR that moved since its analysis opens `/review` with
   the "PR has moved" banner (OD5).
3. **Trust-rule boundary.** Brief §1 trust and wording rules (no numbers or percentages, badges only for
   deterministic facts, banned words, questions not verdicts, collapsed assessment) bind the review module:
   `/review`, `/analyses/**`, their components and agent job kinds `plan`, `pr_context`, `analyse_flow`, `ask`.
   The dashboard shows computed scores, tiers and percentages of scan progress. Rules that still bind it:
   every LLM-written text carries the "LLM" tag (as amended 2026-11-18); every score part names its source
   (`computed` or `LLM`) and basis; deterministic tags come only from scan facts; LLM, GitHub and MemCan text is
   rendered as text only (T12); links are server-built. `scripts/wording-rules.json` checks and rendered-text
   checks exclude the dashboard paths and route `/` only.
4. **Navigation shell.** A left navigation (My work, Review a PR, Settings) wraps `/`, `/review` and `/settings`;
   the Tour and Progress keep the v1 top bar and full width (brief §4 rule 5).
5. **Read-only.** The dashboard never writes to GitHub (no labels, comments, assignments) or MemCan (no TODO
   updates) (OD4).

## Brief amendment (approved; extended with OD17/OD18; WP11-DOCS inserts it into `docs/design-brief.md`)
> Owner-approved amendment — 2026-11-18: GitHub work dashboard. The start page `/` is "My work": PRs and issues
> the user authored, is assigned or is asked to review, and optionally the user's MemCan TODOs, ranked P0–P3 by a
> computed score with visible components, plus pull-request throughput charts. It refreshes on request and once a
> day at a time the user sets (default 06:00), and is read-only towards GitHub and MemCan. The user's agents may
> read it, and everything else the user can read, with a personal access token. The review start page moves to
> `/review`. §1 trust rules apply to the review module; on the dashboard LLM text carries the "LLM" tag and every
> score part names its source. Supersedes "No dashboard" in §2 v1 and the v3+ dashboard item in §2 Later. Plan:
> `docs/v1.1-dashboard-plan.md`.

## Alternatives considered
- Dashboard at `/dashboard`, Home stays at `/`: no deep-link churn, but contradicts OD1 (start page).
- Apply §1 rules to the dashboard and show only tiers: contradicts OD2 and loses the component view the owner
  uses to check rankings.
- Wrap the Tour in the navigation shell: costs width on the review screen (brief §4 rule 1); rejected.

## Consequences
- Web tests that open `/` for Home move to `/review` (plan §F.1 lists them); `ProgressScreen` cancel navigates to
  `/review`.
- `check-wording` gains a path exclusion; a dashboard-specific test asserts the "LLM" tag on every LLM text.
- The Settings screen gains a per-user "Dashboard" section (plan §F.6) and an installation-wide dashboard ranking
  effort next to the model choice.
