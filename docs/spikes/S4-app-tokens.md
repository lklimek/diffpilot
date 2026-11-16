# S4 — GitHub App user tokens and installation tokens

Date: 2026-11-16. App `claudbot` (owner lklimek): contents read, issues write, metadata read, pull_requests
write; **checks not granted**. Installations at test time (17:03 UTC, re-checked before the tests):
`lklimek` (all, 49 repos), `dashpay` (selected; App JWT check: `dash-evo-tool` covered, **`dashpay/platform` NOT
installed**; the user's view also lists `dashpay/tenderdash`). Evidence:
`/data/artifacts/smart-review/2026-11-16/spikes/S4/` — scripts `s4-installation.sh`, `s4-user-token.sh`,
`s4-refresh.sh`, `s4-checks.sh` (reuse `S3/scripts/spike-lib.sh`; run `S3/scripts/redact-app-ids.py` after new
recordings); sanitised `responses/*.json`.

## Questions
User tokens (O4): expiry, refresh rotation, web-flow callback rules. Installation tokens: JWT, minting with
read-only `permissions` and limited `repositories`, 1 h expiry, what they can read (installed repo,
non-installed public repo, private), repository outside the installation, installation status query.

## User-to-server token (device flow, `scope=offline_access` sent by `github-device-login.sh`)
| Run | Result |
|---|---|
| file + `github-authentication-token-expiration` header | access `ghu_` (40 chars) 8 h; refresh `ghr_` (80 chars) ~181 days |
| `r-with-secret-01` refresh (`client_id`, `client_secret`, `grant_type=refresh_token`) | 200, `expires_in 28800`, `refresh_token_expires_in 15638400`, `scope ""`; new refresh token |
| `r-*-03` old access token after refresh | **401 immediately** |
| `r-*-04` replay old refresh token | HTTP **200** with `error: bad_refresh_token` (OAuth errors arrive as 200) |
| `r-*-05` new token after replay | still valid — no token-family revocation on replay |
| `r-without-secret-01` refresh without `client_secret` | works for this device-flow token (as documented) |
| `u12` user token: pending review on `dashpay/platform#5528` (App not installed) | `FORBIDDEN` "Resource not accessible by integration"; reads there OK |
| `u02/u03` `GET /user/installations[/{id}/repositories]` | user-side view of installations and covered repos |

The refresh window slides (each refresh: a new ~181 day refresh token). New pair written atomically (temp file,
0600, `mv`); `GET /user` verified afterwards — stored login valid.

**Web flow (docs, not run)**: authorize `https://github.com/login/oauth/authorize` with `client_id`,
`redirect_uri`, `state`, PKCE `code_challenge` + `code_challenge_method=S256` (`plain` unsupported), optional
`login`, `prompt=select_account`, `allow_signup=false`; exchange `POST /login/oauth/access_token` with
`client_id`, `client_secret`, `code`, `redirect_uri`, `code_verifier`. `redirect_uri` must match one of ≤10
registered callback URLs **exactly** (wildcard matching is opt-in per URL; apps with a single callback URL
enabled before 2026-08-03 have it on — disable it), no extra query parameters; mismatch → `redirect_uri_mismatch`.
Token = intersection of App and user access; `scope` always empty. Sources: docs.github.com
"Generating a user access token for a GitHub App", "About the user authorization callback URL", "Refreshing
user access tokens".

## Installation tokens
| Run | Result |
|---|---|
| `01/02` App JWT (RS256, `iat-60`, `exp+540`) `GET /app`, `/app/installations` | 200 |
| `03` `GET /repos/{o}/{r}/installation` (JWT) | 200 `{id}` covered; **404** `dashpay/platform` |
| `04/05` `GET /orgs/dashpay/installation`, `/users/lklimek/installation` | 200 |
| `10` mint `{repositories:["dash-evo-tool"], permissions:{contents,issues,metadata,pull_requests: read}}` | 201, `expires_at` = mint + **1 h** (fixed, not configurable), selection `selected` |
| `11/12` mint with `platform` (not in installation), alone or with `dash-evo-tool` | **422 for the whole request** "at least one repository … not accessible to the parent installation" |
| `13` `permissions:{checks:read}` | 422 "permissions requested are not granted" |
| `14` mint **without** `permissions` | 201 with **issues/pull_requests write** — `permissions` must always be sent |
| `20` `GET /installation/repositories` | only `dashpay/dash-evo-tool` |
| `21–25` installed repo: PR, files, issues, contents, review comments | 200 |
| `30–35`, `61` **non-installed public `dashpay/platform`**: repo, PR, files, issues, contents, commits, GraphQL | **200** |
| `40–43`, `62` unrelated public `octocat/Hello-World` (REST + GraphQL) | 200 |
| `50–52` search issues/PRs and code on `dashpay/platform` | 200 (search 30/min) |
| `c10–c21` check runs, combined status, Actions runs (no checks/actions permission), both repos | 200 (public data) |
| `70` GraphQL `addPullRequestReview` with the read-only token | `FORBIDDEN` — nothing created |
| `83` lklimek token limited to one public repo → a private lklimek repo | **404** |
| `84` unrestricted lklimek metadata token → same private repo | 200 (private) |
| `85` dashpay token → that private lklimek repo | 404 |
| `90/91` `DELETE /installation/token` | 204; token then 401 |
| `53` rate limit | core/GraphQL 5450/h per installation (shared by all its tokens), search 30/min |

Token format: installation token `ghs_…` with two dots, **390 chars** (JWT-like); App JWT 450 chars. Treat
all tokens as opaque strings without a length limit.

## Findings
1. Installation tokens read **any public repository**, installed or not (REST, GraphQL, search, CI status).
   `repositories` limits only private and write access. For v1 (public only), related repositories need not
   be in the token, and need not be installed.
2. A repository outside the installation makes the whole mint fail (422) — never pass it.
3. Read-only works: explicit `permissions` → read-only token; writes `FORBIDDEN`. Omitting `permissions` gives
   the App's write permissions — a security bug to guard against in code and tests.
4. Lifetime is fixed at 1 h; revocation works and should run at job end.
5. Missing `checks: read` has no effect on public repositories (check runs readable); it is not needed in v1.
6. User tokens: 8 h access, refresh rotates both tokens and kills the old access token at once. Concurrent
   refreshes for one user would invalidate each other's tokens → refresh must be serialised per user, the new
   pair stored atomically before use. OAuth errors come as HTTP 200 + `error`.
7. Commenting requires the App installed on the PR repository (user token `FORBIDDEN` otherwise); reading
   does not.

## Decisions (proposed ADR text)
> **ADR: Installation tokens per job.** The server mints per job from the installation that covers the PR
> repository: `repositories = [PR repo] ∪ (related repos ∩ installation repos)`, `permissions =
> {contents, issues, metadata, pull_requests: read}` always explicit; a mint response whose permissions differ
> from the request is rejected. Related repositories outside the installation are left out and read as public
> data (S4 shows that works; plan §6.2 fallback becomes the normal path). Tokens are revoked
> (`DELETE /installation/token`) when the job ends. Installation status: App JWT
> `GET /repos/{o}/{r}/installation` (200/404), cached briefly; the UI shows "App not installed" and disables
> comments when 404.

> **ADR: User sign-in.** Web flow with `state` + PKCE S256, exact-match callback URLs (wildcard off), expiring
> tokens on. Refresh before expiry, serialised per user; the rotated pair is stored (encrypted) before the old
> one is discarded; `bad_refresh_token` → session ends, user signs in again. Device flow stays a dev/spike tool.

Open for the coordinator: jobs longer than ~55 min outlive the token. Options: (a) cap job runtime < 1 h,
(b) runner asks the server for a fresh token via the job channel and restarts `github-mcp-server`. Not tested.
Also open: PR repo not installed → allow analysis (public reads work with any installation's token) but disable
comments, or refuse the analysis.

## Plan impact
- `JobSpec`: `github_token {value, expires_at}` (opaque string) and the `repositories` it covers; the server
  revokes it when the job ends.
- Token minting module: installation lookup per repo, intersection with installation repos
  (`GET /installation/repositories` or app-side cache), explicit permissions, response check; tests with fake
  GitHub replaying `responses/1*`, `11`–`14`.
- Auth: refresh mutex, atomic store, 200-with-error handling; O2 callback URLs registered without wildcard.
- O2: `checks: read` not needed for v1; `dashpay/platform` install only needed for commenting on its PRs.
- Rate limit: all jobs for one org share 5450/h; WP3 should log `x-ratelimit-remaining` per job.

## Hygiene
All minted installation tokens of the final runs revoked (204 → 401). In a first run aborted by a recording
bug (fixed) two tokens (one from run `14`, with write permissions) were never revoked and expired at 18:05 UTC;
they existed only in that process's memory. User token refreshed twice; stored pair
valid (`GET /user` 200). No pending review left on `dashpay/platform#5528` (`u99-final-verify.json`). Private
repository names and App/client ids are redacted from recordings.
