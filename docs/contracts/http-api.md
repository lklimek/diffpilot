# HTTP API (v1)

Source of truth: `crates/contracts/src/api/**` (types) and `crates/contracts/src/routes.rs` (`ROUTES`). Generated:
`web/src/api/generated/{types,routes}.ts`. This page fixes what the types cannot: authentication, CSRF, status
mapping, query rules and the endpoint × fixture-state inventory.

## Listeners and authentication
- Public listener `SR_LISTEN_ADDR` (behind the reverse proxy): `/healthz`, `/auth/*`, `/api/v1/*`, the web UI.
- Runner listener `SR_RUNNER_LISTEN_ADDR`: `/runner/v1/*` only (`runner-protocol.md`). Neither router serves the
  other's routes (server test).
- `session`: session cookie (Secure, HttpOnly, SameSite=Lax; name set by WP2-AUTH), opaque token stored hashed.
- `session_csrf`: session plus header `X-CSRF-Token` equal to `MeResponse.csrf_token`. Every non-GET public
  route is `session_csrf` (contract test). CSRF applies to cookie-authenticated routes only; runner routes use
  bearer tokens.
- `GET /api/v1/me` is public: signed out it answers `200 {"user": null, "csrf_token": null}`, not 401.

## Routes
| Method | Path | Auth | Request | Response |
|---|---|---|---|---|
| GET | `/healthz` | public | – | 204 |
| GET | `/auth/github/login` | public | – | 302 to GitHub |
| GET | `/auth/github/callback` | public | `code`, `state` | 302 to `/` or `/?login_error=<LoginError>` |
| POST | `/auth/logout` | session_csrf | `LogoutRequest` | 204 |
| GET | `/api/v1/me` | public | – | `MeResponse` |
| POST | `/api/v1/pr/resolve` | session_csrf | `ResolvePrRequest` | `ResolvePrResponse` |
| POST | `/api/v1/analyses` | session_csrf | `CreateAnalysisRequest` | 201 `AnalysisSummary` |
| GET | `/api/v1/analyses/{id}` | session | – | `AnalysisSummary` |
| GET | `/api/v1/analyses/{id}/events` | session | – | SSE (`sse-events.md`) |
| POST | `/api/v1/analyses/{id}/cancel` | session_csrf | – | `AnalysisSummary` |
| POST | `/api/v1/analyses/{id}/retry` | session_csrf | – | `AnalysisSummary` |
| GET | `/api/v1/analyses/{id}/tour` | session | – | `Tour` (409 `analysis_not_finished` before) |
| GET | `/api/v1/analyses/{id}/lines` | session | `RangeQuery` | `LinesResponse` (409 `analysis_not_finished` before the analysis is ready) |
| GET | `/api/v1/analyses/{id}/diff` | session | `RangeQuery` | `DiffResponse` (409 `analysis_not_finished` before the analysis is ready) |
| GET / PUT | `/api/v1/analyses/{id}/review-state` | session / session_csrf | – / `ReviewState` | `ReviewState` / 204 (PUT: 409 `analysis_not_finished` before `finished`) |
| GET | `/api/v1/analyses/{id}/questions` | session | – | `AskQuestion[]` |
| POST | `/api/v1/analyses/{id}/steps/{step_id}/questions` | session_csrf | `AskRequest` | 202 `AskQuestion` |
| GET / POST | `/api/v1/analyses/{id}/feedback` | session / session_csrf | – / `FeedbackRequest` | `Feedback[]` / `Feedback` |
| GET | `/api/v1/analyses/{id}/pending-review` | session | – | `PendingReview` |
| POST | `/api/v1/analyses/{id}/pending-review/comments` | session_csrf | `CreateCommentRequest` | 201 `CreateCommentResponse` |
| PATCH | `…/pending-review/comments/{comment_id}` | session_csrf | `UpdateCommentRequest` | `PendingComment` |
| DELETE | `…/pending-review/comments/{comment_id}` | session_csrf | – | 204 |
| GET | `/api/v1/settings` | session | – | `Settings` |
| PUT | `/api/v1/settings/language` | session_csrf | `SetLanguageRequest` | 204 |
| GET | `/api/v1/repos/{owner}/{name}/settings` | session | – | `RepoSettings` (read-only; 409 `analysis_not_finished` while not known) |

## Rules
- **Ranges** (`lines`, `diff`): query `path`, `side` (`head` | `base`), `from`, `to`; `1 ≤ from ≤ to`,
  `to − from + 1 ≤ RangeQuery::MAX_LINES` (2000) else `range_too_large`; `to` is clamped to the file. The API
  never serves whole files (brief §7). `side=base` serves the base commit (deleted or emptied files, removed
  blocks). Paths outside the PR's files and impact/context files of the head tree are allowed; generated
  material classified "not rendered" answers `not_rendered`.
- **Deleted blocks** (`LinesResponse.deleted`): every deletion marker in the range is reported with its
  `total_lines`; its `lines` hold at most `RangeQuery::MAX_LINES` removed lines per response, shared by the
  blocks in order (a block past the budget has empty `lines`). `lines.len() < total_lines` means truncated: read
  the rest with `side=base` from `base_start + lines.len()`.
- **Analysis base**: the analysed diff is GitHub's three-dot comparison, from the merge base of the PR's base
  tip and head to the head. An analysis is ready to serve code when its stored base is the merge base of its
  stored base and head, both commits being in the server's mirror (fetched on demand); no job runs before
  that. `lines` and `diff` answer `analysis_not_finished` while it is not ready.
- **Commenting** (ADR-006): `LinesResponse.comments` is `in_diff {line_thread_ranges}` or `not_in_diff`; a create
  request for a `not_in_diff` target is `position_rejected` without a GitHub call. `commenting_available` is
  false (and comment routes answer `app_not_installed`) when the GitHub App is not installed on the PR repo or is
  unusable as configured; it is read per request (installation lookup). Every write (create, edit, delete) is
  serialised per (user, PR) and first compares the PR head with the analysis head (`pr_head_changed`); edit and
  delete act only on the user's own pending comments of this PR (any other id is `not_found`).
- **Head guarantee of a new comment: verified after the write.** GitHub's thread mutation takes no commit, so the
  preflight alone cannot bind a comment to the analysed head: the author may push in between. The server reads
  the new comment's commit from the mutation's answer; when it is not the analysed head (or missing), it deletes
  the comment again and answers `pr_head_changed`. If that deletion fails, the answer is also `pr_head_changed`,
  with a message saying the comment may remain in the pending review; the idempotency key then stays busy. If
  the review was published before the deletion, the published comment is removed and the answer is
  `comment_published` (nothing of the attempt remains; the key is freed).
- **Edit and delete of a comment published meanwhile** (accepted residual risk, ADR-006): GitHub has no
  conditional edit or delete, so the user may publish the review (e.g. in another tab) between the server's
  pending check and the mutation; the change then applies to the published comment. The server reads the
  comment's and review's state from the mutation's answer and reports this as `comment_published` (the change
  was applied; never a silent success).
- **Pending-review list** (`PendingReview`): every pending review of the user on the PR is merged; at most 50
  pending reviews with 1000 comments each are read (pages of 10 and 100). Beyond that `incomplete` is true. A
  list cut at that cap (or at a next page GitHub reports without a cursor) is never taken as proof that a comment does not exist: edit and delete then look the
  comment up by id.
- **No installation** (ADR-005 addendum): analysis still runs; `PrContextFacts.related_work_checked = false`
  (UI: "Related work: not checked"); commenting disabled.
- **CI status** (`PrContextFacts.ci`): read from the head commit's check runs with the user token of the analysis
  creator at ingest. `none` = read, no check runs; `not_checked` = the check runs could not be read (no token,
  GitHub error, rate limit, or more than 1000 check runs, the read cap). `CiCheck.failing_tests` holds names only
  from formats that mark a failure unambiguously (libtest `test <name> ... FAILED` and its `failures:` list,
  pytest `FAILED <file>::<test>`), at most 20 names of at most 200 characters; any other summary gives none (the
  check still counts as failing, with its link). `CiCheck.url` and `GithubThread.url` are always `https://github.com/…`: any other
  URL GitHub reports is dropped (`CiCheck.url` null; a thread links to its pull request instead).
- **Review threads** (`PrContextFacts.threads_status`, `Tour.threads`): read with the same token at ingest.
  `checked` = every thread is listed; `partial` = only the first ones are listed (the 1000-thread read cap, or
  a next page without a cursor); `not_checked` = they could not be read (`Tour.threads` empty, which says
  nothing about their existence).
- **Idempotency**: `CreateCommentRequest.idempotency_key` per (user, key) returns the first response. The same key
  with another request (other analysis, target or body) is `conflict`; so is a repeat while the first attempt's
  outcome is unknown: the comment may exist, the pending-review list shows it. A failed write frees the key only
  when the answer proves nothing was written; "resolved data" = `data` with a non-null root field:

  | Answer to the thread mutation | Outcome | Key |
  |---|---|---|
  | GraphQL errors without `data`, or with `data: null` (refused before execution, e.g. validation) | error (`app_not_installed` for `FORBIDDEN`) | freed |
  | `FORBIDDEN` with the mutation field null | `app_not_installed` | freed |
  | `thread: null` without errors | `position_rejected` | freed |
  | HTTP 401, 429, 403 rate limit or other 4xx, without resolved data; request never sent | error | freed |
  | any error next to resolved data (including `FORBIDDEN` and a secondary rate limit carrying data) | unknown (`github_unavailable`) | busy |
  | any other error with the mutation field null | unknown | busy |
  | no answer, 5xx, unreadable answer, `data` without the comment | unknown | busy |
  | comment placed on another head, deleted again | `pr_head_changed` | freed |
  | comment placed on another head, its review published meanwhile, deleted again | `comment_published` | freed |
  | comment placed on another head, deletion failed | `pr_head_changed` (may remain) | busy |

- **Settings**: `repos` lists the tenant's repositories with at least one analysis; each opens
  `/api/v1/repos/{owner}/{name}/settings` (owner and name match case-insensitively; any other repository is
  `not_found`). Its rules, related repositories and profile source come from the newest stored pre-pass of the
  repository; without one the route answers `analysis_not_finished` ("Not known yet …"), never invented values.
  `llm_status` is the newest LLM fact: the outcome of the most recent call the runners reported, or a credential
  reported unusable after it (`not_working`, threat model T13); `null` before the first call. Installations are
  read from GitHub with the user token (`GET /user/installations`, suspended ones left out); a GitHub failure is
  `github_unavailable`, never an empty list. `PUT …/settings/language` accepts only `en` and `pl`.
- **Review state**: `PUT` replaces the user's whole state (idempotent; repeated ticks are stored once); concurrent
  writes leave one of them whole. Every ticked step and the last viewed block must be in the analysis's Tour
  (quiet rows only when the Tour shows them), else `invalid_request` and nothing is stored.
- **Classic diff** (`diff`): unified hunks of the file that overlap the range on `side`, three context lines,
  hunks at most six unchanged lines apart merged (as `git diff -U3`); lines of `side` outside the range ±3 are
  left out, changed lines of the other side are shown whole, at most `RangeQuery::MAX_LINES` of them, else
  `range_too_large`. A file of the pinned tree outside the pull request diff has no hunks; a binary file is
  `not_rendered`.
- Ids in paths are UUIDs (`analysis`, `question`) or server ids (`f1.2` steps); unknown or another tenant's →
  `not_found` (no existence leak).

## Errors
Body `ApiError {code, message}`; `message` is plain text for the user, never internal error text (server
`ApiFailure` logs the chain: `error!` for 5xx, `debug!` otherwise).

| Code | Status | When |
|---|---|---|
| `invalid_request` | 400 | malformed JSON, query or path; a review state naming a step or block not in the Tour |
| `not_a_pr_url` | 400 | resolve: not a GitHub PR URL |
| `range_too_large` | 400 | range over 2000 lines; a diff with over 2000 changed lines of the other side |
| `unauthenticated` | 401 | no or expired session |
| `login_not_allowed` | 403 | login not in the tenant allowlist |
| `csrf_rejected` | 403 | missing or wrong `X-CSRF-Token` |
| `not_found` | 404 | unknown or not visible |
| `conflict` | 409 | state conflict (e.g. analysis already live for this head) |
| `analysis_not_finished` | 409 | tour, questions, review-state writes before `finished`; lines, diff before the analysis is ready (stored base is the merge base); repository settings without a stored pre-pass |
| `pr_head_changed` | 409 | comment write when the PR head moved (before or during the write) |
| `comment_published` | 409 | edit or delete applied to a comment whose review was published meanwhile; or a new comment on a moved head removed from a review published meanwhile |
| `private_repository` | 422 | v1 is public repositories only |
| `app_not_installed` | 422 | comment routes without the App on the PR repo |
| `position_rejected` | 422 | GitHub silently rejected (`thread: null`) or target not commentable |
| `not_rendered` | 422 | file classified as not rendered; diff of a binary file |
| `internal` | 500 | anything else (no detail) |
| `github_unavailable` | 502 | GitHub error or unexpected answer |

## Endpoint × fixture-state inventory
Fixture PR `acme/node-client#482` (`fixtures/pr-482/`): `api-seed/default/` plus merge patches per state.
"–" = identical to `default`.

| Endpoint | default | wrong | missed | noq | noprofile | resume |
|---|---|---|---|---|---|---|
| `me` | signed in | – | – | – | – | – |
| `pr/resolve` | no analysis | – | – | – | – | existing finished analysis |
| `analyses/{id}` | finished, 5 flows | – | – | – | – | – |
| `tour` | 5 flows / 17 steps | step 3.1 rated high (confidently wrong) | – | no `question` annotations, no assessments | `project_profile.sources = []` | – |
| `lines`, `diff` | ranges of #482 | – | `src/rpc/client.rs` 96 changed, no annotation | – | – | – |
| `review-state` | empty | – | – | – | – | ticks f2.1, last viewed step f1.5 |
| `questions` | none | – | – | – | – | one answered on f1.5 |
| `feedback` | none | `wrong` + `overrated` on step 3.1 | `missed_by_llm` line 96 | – | – | – |
| `pending-review` | empty | – | one LINE comment on line 96 | – | – | – |
| `settings` | working, en, 1 repo | – | – | – | – | – |
