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
| GET / PUT | `/api/v1/analyses/{id}/review-state` | session / session_csrf | – / `ReviewState` | `ReviewState` / 204 |
| GET | `/api/v1/analyses/{id}/questions` | session | – | `AskQuestion[]` |
| POST | `/api/v1/analyses/{id}/steps/{step_id}/questions` | session_csrf | `AskRequest` | 202 `AskQuestion` |
| GET / POST | `/api/v1/analyses/{id}/feedback` | session / session_csrf | – / `FeedbackRequest` | `Feedback[]` / `Feedback` |
| DELETE | `…/feedback/{feedback_id}` | session_csrf | – | 204 |
| GET | `/api/v1/analyses/{id}/pending-review` | session | – | `PendingReview` |
| POST | `/api/v1/analyses/{id}/pending-review/comments` | session_csrf | `CreateCommentRequest` | 201 `CreateCommentResponse` |
| PATCH | `…/pending-review/comments/{comment_id}` | session_csrf | `UpdateCommentRequest` | `PendingComment` |
| DELETE | `…/pending-review/comments/{comment_id}` | session_csrf | – | 204 |
| GET | `/api/v1/settings` | session | – | `Settings` |
| PUT | `/api/v1/settings/language` | session_csrf | `SetLanguageRequest` | 204 |
| GET | `/api/v1/repos/{owner}/{name}/settings` | session | – | `RepoSettings` (read-only) |

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
  false (and comment routes answer `app_not_installed`) when the GitHub App is not installed on the PR repo.
- **No installation** (ADR-005 addendum): analysis still runs; `PrContextFacts.related_work_checked = false`
  (UI: "Related work: not checked"); commenting disabled.
- **Idempotency**: `CreateCommentRequest.idempotency_key` per (user, key) returns the first response.
- **Feedback** (CCR-F1): one entry per (user, analysis, target, value); several values may coexist on a target.
  Posting a value again replaces its note (same id); `overrated` and `underrated` exclude each other (posting one
  removes the other, one transaction); `DELETE …/feedback/{feedback_id}` removes one value of the user's own
  entries (`not_found` otherwise). `missed_by_llm` is recorded only by the server (a comment on a changed line
  without annotation) and rejected from clients (`invalid_request`, it is not a `UserFeedbackValue`). A step or
  annotation target must exist in the analysis's flows, a line target needs a path and a line ≥ 1, the note at
  most `FeedbackRequest::MAX_NOTE_CHARS` (2000) characters, else `invalid_request`; a blank note is stored as
  none. Feedback is stored locally at once and written to MemCan asynchronously; no response claims it is in
  MemCan.
- **Settings**: `repos` lists the tenant's known repositories (from `repos`); each opens
  `/api/v1/repos/{owner}/{name}/settings`.
- Ids in paths are UUIDs (`analysis`, `question`) or server ids (`f1.2` steps); unknown or another tenant's →
  `not_found` (no existence leak).

## Errors
Body `ApiError {code, message}`; `message` is plain text for the user, never internal error text (server
`ApiFailure` logs the chain: `error!` for 5xx, `debug!` otherwise).

| Code | Status | When |
|---|---|---|
| `invalid_request` | 400 | malformed JSON, query or path |
| `not_a_pr_url` | 400 | resolve: not a GitHub PR URL |
| `range_too_large` | 400 | range over 2000 lines |
| `unauthenticated` | 401 | no or expired session |
| `login_not_allowed` | 403 | login not in the tenant allowlist |
| `csrf_rejected` | 403 | missing or wrong `X-CSRF-Token` |
| `not_found` | 404 | unknown or not visible |
| `conflict` | 409 | state conflict (e.g. analysis already live for this head) |
| `analysis_not_finished` | 409 | tour, questions before `finished`; lines, diff before the analysis is ready (stored base is the merge base) |
| `pr_head_changed` | 409 | comment write when the PR head moved |
| `private_repository` | 422 | v1 is public repositories only |
| `app_not_installed` | 422 | comment routes without the App on the PR repo |
| `position_rejected` | 422 | GitHub silently rejected (`thread: null`) or target not commentable |
| `not_rendered` | 422 | file classified as not rendered |
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
