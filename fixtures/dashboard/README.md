# Dashboard fixtures (v1.1)

The fictional data of the approved dashboard mockup (`/data/artifacts/diffpilot/2026-11-18/dashboard-mockup/`,
`dashboard.js`) as API responses and scan facts. Everything is synthetic: accounts, repositories (`acme/*`), ids and
tokens are fake. The signed-in user is `alice` (as `fixtures/pr-482`); `NC-482` is that fixture's pull request,
analysed at its head `52d2da1` and moved since. Consistency and the wording rules are checked by
`crates/contracts/tests/dashboard.rs` and `crates/contracts/tests/fixtures.rs`. GitHub answers for the fake GitHub
live in `github/` (WP11-GHFAKE).

| Path | Content |
|---|---|
| `api/summary.json` | `DashboardSummary` (R1): a finished manual refresh at 08:12, ranked at 08:15; schedule on at 06:00 Europe/Warsaw; 12 weeks of history events; three excluded items listed (as with `?excluded=true`) |
| `api/items.json` | `ItemPage` (R1b) of that snapshot: the 14 mockup items, every score final, in rank order |
| `api/items-provisional.json` | the same items while a later refresh ranks: `WUI-320` new (`unjudged`, named estimates), `NC-482`, `CON-1534`, `PLAT-1210` changed (`provisional`, inherited LLM parts) |
| `api/refresh-scanning.json`, `refresh-ranking.json`, `refresh-failed.json` | `RefreshStatus` (R2b, SSE `refresh`): scanning PR details 23 of 41; ranking 10 of 14; failed on GitHub's rate limit (retry at 10:41) |
| `api/settings.json` | `DashboardSettingsView` (R6): six repository rows, one linked login, one extra MemCan project, `acme/legacy-tools` not visible to the token |
| `api/access-tokens.json`, `api/access-token-created.json` | `AccessToken[]` (R8) and `CreatedAccessToken` (R9; the secret is a fake placeholder) |
| `scan/scan.json` | scan facts of the pull requests and issues in the field names of the daily-priorities `dp-scan.json` (subset, no score parts) |
| `scan/todos.json` | MemCan `list_todos` answers per project (`{todos, has_more}`, S11-4) for the two TODO items |

Scores are in tenths of a point: each item's `total_tenths` is the sum of its parts; the tier follows plan §C
(P0 alarm, P1 ≥ 650, P2 350–<650, P3 < 350 tenths).
Item keys use the normative form of `docs/contracts/http-api.md` "Item keys" (`acme~wallet-ui~pr~320`,
`memcan~<uuid>`); `updated_at` is GitHub's `updatedAt` for pull requests and issues and the first sight of the
current values for TODOs.
