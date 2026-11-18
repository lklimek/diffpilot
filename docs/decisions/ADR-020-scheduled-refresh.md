# ADR-020: Scheduled dashboard refresh and the GitHub token pair's lifetime

- Status: accepted (owner, 2026-11-18, OD18); changes the token-pair lifetime of the ADR-007 addendum.
  Plan `docs/v1.1-dashboard-plan.md` §A.5, WP11-SCHED.

## Context
The owner wants a daily automatic refresh (default on, 06:00 local time, configurable, can be switched off).
Nothing may run outside Docker. The ADR-007 addendum keeps a user's GitHub token pair only while that user has a
live session (7 days absolute, 24 h idle); `db::repo::sessions::delete_expired` removes pairs left without
sessions. A 06:00 run after a quiet weekend would find no pair.

## Decision
1. **In-process scheduler** (`server::dashboard::schedule`): a 60 s tick evaluates, per user, the due rule of plan
   §A.5 from stored settings and run records; no timers are persisted, so restarts lose nothing. One run per user
   per local date; a missed run is caught up once at the first tick on the same local date unless a refresh already
   succeeded after the scheduled instant. Scheduled runs use the manual refresh pipeline and its one-live-refresh
   rule; at most one scheduled refresh runs server-wide, users ordered by due instant plus a per-user 0–9 min offset.
2. **Time zone** per user (IANA, `chrono-tz`), stored from the browser at the first dashboard visit, editable in
   Settings; without it the schedule does not run. DST: gap → first valid instant after; overlap → first occurrence.
3. **Token pair lifetime:** the pair is kept while the user has a live session **or** the schedule is on. Sign-out of
   the last session keeps it when the schedule is on (Settings says so); "sign out everywhere", removal from the
   allow-list, a rejected refresh or an expired refresh token delete it as in v1. The pair stays AES-GCM encrypted
   and is refreshed through the v1 path (serialised per user). Without a usable pair the run records
   `needs_sign_in`, shown on the dashboard until a later refresh succeeds.
4. **Ranking** of scheduled refreshes follows the token-saving rule (plan §B.4) and waits behind analysis jobs.

## Alternatives considered
| Alternative | Why not |
|---|---|
| Host cron or a separate container | outside the server's state and auth; owner rule "nothing outside Docker" and one process is simpler |
| Persisted next-run timestamps | redundant with the due rule; drifts with time-zone and DST changes |
| Use an installation token for scheduled scans | cannot search the user's work across installations (ADR-017) |
| Run only while a session is live | the 06:00 run would fail after every idle day |
| Catch up every missed date | stale rankings for past days are worthless; one run for today is enough |

## Consequences
- The refresh token (~181 days, sliding) stays on the server while the schedule is on (threat-model T21: a stored
  long-lived credential without a live session; mitigations: encryption at rest, allow-list check before each run,
  sign-out everywhere deletes it).
- `delete_expired` and sign-out change in `crates/db/src/repo/sessions.rs` and `crates/server/src/auth/` (CCR).
