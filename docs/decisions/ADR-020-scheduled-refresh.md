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
   rule (a start supersedes a ranking; only a scanning refresh blocks; `docs/contracts/http-api.md` "Refresh states"); at most one scheduled refresh runs server-wide, users ordered by due instant plus a per-user 0–9 min offset.
2. **Time zone** per user (IANA; jiff's bundled tz database, addendum "Time library"), stored from the browser at the first dashboard visit, editable in
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

## Addendum (2026-11-18, WP11-SCHED review; coordinator decision for the owner)
- **Inactivity limit.** Each scheduled run rotates the refresh token, so its ~181-day expiry slides and the kept pair
  would otherwise never lapse. The schedule keeps the pair only while the user signed in within the last
  `SCHEDULE_PAIR_KEEP_DAYS` = 30 days (`users.updated_at`, set at sign-in and by a language change; a session lasts
  at most 7 days from it, so the pair ends at most 30 days after the user was last seen). Past that the per-minute
  sweep deletes the pair, the next scheduled run records `needs_sign_in` ("Sign in again to keep the daily refresh
  running") and nothing runs until a sign-in. No migration: the bound is derived from the existing column.
- **Failure isolation.** A user whose run fails on the server (database error, panic, unreadable settings row) is
  skipped for that local date (recorded `failed`, or remembered in-process when even that write fails); other users
  are not held up.
- **Restarts.** The scheduler stops before starting another run once shutdown is signalled. At start-up a run still
  recorded `started` whose refresh never started or whose scan the restart interrupted is recorded `failed`
  ("interrupted by a server restart"); that record does not count for its date, so the day's catch-up rule decides
  again.

## Addendum (2026-11-18): time library
- **Decision.** chrono and chrono-tz are replaced by jiff 0.2 workspace-wide. RustSec marks both unmaintained
  (RUSTSEC-2026-0543, RUSTSEC-2026-0555) and recommends jiff, which also covers IANA zones and DST in one crate.
- **Zones.** jiff's bundled IANA database only (feature `tzdb-bundle-always`, no system tzdata), so zone names and
  rules do not depend on the minimal server image. The rules ship in `jiff-tzdb` (pinned in `Cargo.lock`): run
  `cargo update -p jiff-tzdb` and redeploy on each IANA release; this move takes the data from 2025b (chrono-tz
  0.10.4) to 2026f, which shifts local times of about 15 zones in 2025–2027 (e.g. America/Vancouver). `server::dashboard::zone` accepts exact names only (jiff's lookup
  is case-insensitive; chrono-tz was not), links included, `Etc/Unknown` refused. The DST rules above, the chart
  window (midnight gaps such as America/Santiago) and the local-day judge cap keep their behaviour and tests.
- **Wire and storage unchanged.** `contracts::time::Timestamp` wraps `jiff::Timestamp` and serialises as before:
  RFC 3339 with `Z` and the fewest of 0, 3, 6 or 9 fraction digits (jiff's own serde drops trailing zeros).
  Stored columns keep fixed-width microseconds with `Z`; installation token expiries keep the `+00:00` form. No
  migration; JSON Schemas and TS types are unchanged (`date-time` strings).
- **Remaining chrono.** `rmcp` (MemCan client) depends on chrono, so it stays in `Cargo.lock`; no workspace code
  uses it.

## Amendment (2026-11-22, ADR-023 Watch, WP13-WATCH)
- **Watching keeps the pair.** The token pair is also kept without a live session while the user watches a pull
  request (`pr_watches` row not `ended`, in a tenant the user is allowed in), within the same 30-day inactivity bound
  as the schedule (`users.updated_at`). Ending the last watch, or the bound passing, lets the next sweep delete the
  pair; the user's watches then pause ("Watching paused: sign in again") until a probe reads GitHub again.
- **Same scheduler task.** The watch probe runs in the scheduler loop at most once per 60 s tick and probes each
  watch every 5 minutes. A watched pull request is read by that probe, so the plan's extra daily scheduled read of
  watched PRs is not added (it would only repeat the probe).
