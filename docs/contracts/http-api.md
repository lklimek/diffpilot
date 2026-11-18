# HTTP API (v1)

Source of truth: `crates/contracts/src/api/**` (types) and `crates/contracts/src/routes.rs` (`ROUTES`). Generated:
`web/src/api/generated/{types,routes}.ts`. This page fixes what the types cannot: authentication, CSRF, status
mapping, query rules and the endpoint × fixture-state inventory.

## Listeners and authentication
- Public listener `SR_LISTEN_ADDR` (behind the reverse proxy): `/healthz`, `/auth/*`, `/api/v1/*`, the web UI.
- Runner listener `SR_RUNNER_LISTEN_ADDR`: `/runner/v1/*` only (`runner-protocol.md`). Neither router serves the
  other's routes (server test).
- `session`: session cookie (`__Host-sr_session`, Secure, HttpOnly, SameSite=Lax, Path=/; `sr_session` without
  Secure only when `SR_PUBLIC_URL` is http and no trusted proxy forwards https), opaque token stored hashed. A
  session lasts 7 days, ends after 24 h unused, and only while its user stays on the tenant's allowlist (ADR-007
  addendum). `POST /auth/logout` ends this session (`everywhere`: all of the user's); the stored GitHub tokens end
  with the user's last session.
- `session_csrf`: session plus header `X-CSRF-Token` equal to `MeResponse.csrf_token`. Every non-GET public
  route is `session_csrf` (contract test). CSRF applies to cookie-authenticated routes only; runner routes use
  bearer tokens.
- `GET /api/v1/me` is public: signed out it answers `200 {"user": null, "csrf_token": null}`, not 401.
- `read` (v1.1, ADR-019): session, or a personal access token with scope `read` (`Authorization: Bearer
  srp_<id>_<secret>`; `RouteAuth::SessionOrToken(TokenScope::Read)`, named `read` in `routes.ts`). Only `GET` routes
  of the public listener are `read` (contract test); a token never lists or mints tokens (R8 is `session`). Rules
  of the token check: "Dashboard (v1.1)" below.

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
| GET | `/api/v1/analyses/{id}` | read | – | `AnalysisSummary` |
| GET | `/api/v1/analyses/{id}/events` | read | – | SSE (`sse-events.md`) |
| PUT | `/api/v1/settings/llm` | session_csrf | `ModelChoice` | `LlmSelection` |
| PUT | `/api/v1/settings/llm/ask-effort` | session_csrf | `SetAskEffortRequest` | `LlmSelection` (Ask jobs' reasoning effort, default `medium`; every other job uses `current`; `LlmSelection.ask_effort` reports it; `changed_by`/`changed_at` are shared with the model choice) |
| POST | `/api/v1/analyses/{id}/pause` | session_csrf | – | `AnalysisSummary` |
| POST | `/api/v1/analyses/{id}/resume` | session_csrf | – | `AnalysisSummary` |
| POST | `/api/v1/analyses/{id}/cancel` | session_csrf | – | `AnalysisSummary` |
| POST | `/api/v1/analyses/{id}/retry` | session_csrf | – | `AnalysisSummary` |
| GET | `/api/v1/analyses/{id}/tour` | read | – | `Tour` (409 `analysis_not_finished` until plan and PR context are accepted) |
| GET | `/api/v1/analyses/{id}/lines` | read | `RangeQuery` | `LinesResponse` (409 `analysis_not_finished` before the analysis is ready) |
| GET | `/api/v1/analyses/{id}/diff` | read | `RangeQuery` | `DiffResponse` (409 `analysis_not_finished` before the analysis is ready) |
| GET / PUT | `/api/v1/analyses/{id}/review-state` | read / session_csrf | – / `ReviewState` | `ReviewState` / 204 (PUT: 409 `analysis_not_finished` before `finished`) |
| GET | `/api/v1/analyses/{id}/questions` | read | – | `AskQuestion[]` |
| POST | `/api/v1/analyses/{id}/steps/{step_id}/questions` | session_csrf | `AskRequest` | 202 `AskQuestion` |
| GET / POST | `/api/v1/analyses/{id}/feedback` | read / session_csrf | – / `FeedbackRequest` | `Feedback[]` / `Feedback` |
| DELETE | `…/feedback/{feedback_id}` | session_csrf | – | 204 |
| GET | `/api/v1/analyses/{id}/pending-review` | read | – | `PendingReview` |
| POST | `/api/v1/analyses/{id}/pending-review/comments` | session_csrf | `CreateCommentRequest` | 201 `CreateCommentResponse` |
| PATCH | `…/pending-review/comments/{comment_id}` | session_csrf | `UpdateCommentRequest` | `PendingComment` |
| DELETE | `…/pending-review/comments/{comment_id}` | session_csrf | – | 204 |
| GET | `/api/v1/settings` | read | – | `Settings` |
| PUT | `/api/v1/settings/language` | session_csrf | `SetLanguageRequest` | 204 |
| GET | `/api/v1/repos/{owner}/{name}/settings` | read | – | `RepoSettings` (read-only; 409 `analysis_not_finished` while not known) |

## Rules
- **Ranges** (`lines`, `diff`): query `path`, `side` (`head` | `base`), `from`, `to`; `1 ≤ from ≤ to`,
  `to − from + 1 ≤ RangeQuery::MAX_LINES` (2000) else `range_too_large`; `to` is clamped to the file. The API
  never serves whole files (brief §7). `side=base` serves the base commit (deleted or emptied files, removed
  blocks). Paths outside the PR's files and impact/context files of the head tree are allowed; generated
  material and binary files the pre-pass lists as "not rendered" answer `not_rendered` (422); before the pre-pass
  is stored `analysis_not_finished`.
- **Unexplained reason** (`Tour.quiet.unexplained[].reason`, optional, closed enum): set only for lines the
  server did not send to the LLM, a deterministic fact; v1 value `too_large_for_prompt` (a file too large for any
  plan prompt, or a flow whose prompt is over budget). A file's ranges with and without a reason are separate
  entries. Lines the LLM was asked about but did not explain carry no reason.
- **Analysis stage**: `AnalysisSummary.state` of a `planning`, `running` or `paused` analysis carries an optional
  `stage {kind, flow?, started_at}` (the live job's stage; `started_at` null = waiting for the runner, absent =
  no live job); rules in `sse-events.md`. Example: `{"state": "running", "done": 1, "total": 5, "stage": {"kind":
  "flow", "flow": {"index": 2, "title": "Default timeout 30 s → 5 s"}, "started_at": "2026-11-17T09:30:00Z"}}`.
- **Cancel and retry**: cancel applies to a planning, running, paused or failed analysis (else `conflict`): state
  `cancelled`, unfinished jobs cancelled and stored results discarded; both PR sessions retained (a `finished`
  analysis running again for a flow request is `finished` again instead, results kept). Retry applies to a
  failed analysis only (else `conflict`): state `planning`, stored results kept, failed stages submitted again.
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
- **CI status** (`PrContextFacts.ci`): read at ingest with the analysis creator's user token from two independent
  endpoints: check runs and combined commit statuses (latest per context). Each source is capped at 10 pages
  of 100; refusal (including 403/404), failure, an early empty page or a cut read leaves that source absent
  (not checked), never zero. `check_runs` and `statuses`, when present, each contain exact counts
  `{passed, failed, pending, skipped, neutral}`. `passed` means only success; skipped and neutral are separate.
  `total` and `failing` include both read sources. State precedence: failing, pending, then passing only if
  both sources were read, at least one result exists and every result succeeded; otherwise `no_failures`
  for concluded nonfailing results, `none` for two read empty sources, `not_checked` for no results with any
  unread source. Old stored facts can lack both count fields; clients must not claim “all passing” from such
  a legacy `state` alone. Example: `{"state":"pending","total":6,"failing":[],"check_runs":{"passed":1,
  "failed":0,"pending":0,"skipped":4,"neutral":0},"statuses":{"passed":0,"failed":0,"pending":1,
  "skipped":0,"neutral":0}}`: “1 passed, 4 skipped; 1 status pending. Check runs and commit statuses read.”
  A single absent source is named explicitly as “not checked”.
  GitHub documents [checks read permission](https://docs.github.com/en/rest/checks/runs#list-check-runs-for-a-git-reference)
  and [commit statuses read permission](https://docs.github.com/en/rest/commits/statuses#get-the-combined-status-for-a-specific-reference)
  separately; contents permission alone is not substituted for these endpoint permissions.
  `CiCheck.failing_tests` extracts only explicit libtest/pytest failure names (at most 20 of 200 characters).
  `CiCheck.url` and `GithubThread.url` stay restricted to `https://github.com/…`; other check URLs become null.
- **Test diff churn**: `lines_removed_from_tests` counts removed diff lines in deterministically classified
  test files, including in-place replacements; `lines_added_to_tests` counts added diff lines on the head
  side. Neither counts assertions or functions, or establishes weakened coverage. Render together as
  “4 test lines changed or removed, 57 added”. An absent added count in an old stored pre-pass is unknown,
  never zero. Pure additions and pure deletions are counted on their respective sides.
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

- **Feedback** (CCR-F1): one entry per (user, analysis, target, value); several values may coexist on a target.
  Posting a value again replaces its note (same id); `overrated` and `underrated` exclude each other (posting one
  removes the other, one transaction); `DELETE …/feedback/{feedback_id}` removes one value of the user's own
  entries (`not_found` otherwise). `missed_by_llm` is recorded only by the server (a comment on a changed line
  without annotation; before the analysis is `finished` only inside a step of an analysed flow, since the LLM has
  not read the other flows yet) and rejected from clients (`invalid_request`, it is not a `UserFeedbackValue`). A step or
  annotation target must exist in the analysis's flows under its canonical Tour id (`f3.1`, `f3.1.a2`; aliases
  such as `f3.01` are rejected), a line target must be a changed line of the analysed diff on its side (added on
  `head`, removed on `base`), the note at most `FeedbackRequest::MAX_NOTE_CHARS` (2000) characters, else
  `invalid_request`; a blank note is stored as none. Feedback is stored locally at once; only the fixed value and public target facts are written to MemCan
  asynchronously. Private notes never leave the local user-scoped row; `Feedback.memory` is the state of the current revision's copy there (`pending`, `written` =
  MemCan confirmed it and no earlier write of unknown outcome can still land, `failed` = the last attempt failed,
  retried on a back-off). Only `written` may be shown as "saved to calibration memory".
- **Ask** (`…/steps/{step_id}/questions`): only for an accepted step in a review-ready or `finished` analysis (else `analysis_not_finished`) whose PR data was not explicitly
  removed (else `conflict`); `step_id` is a canonical step id of the analysis's Tour (`f1.5`; aliases
  such as `f1.05` and unknown steps are `not_found`); `text` is trimmed, 1 to `AskRequest::MAX_TEXT_CHARS` (2000)
  characters, else `invalid_request`, and stored and shown as text. No idempotency key: each request is a new
  question. Limits (`conflict` with a plain message): 100 questions per user per analysis, 30 per user per hour. The
  question runs as an `ask` job claimed before batch jobs; one job runs per PR and credential, so a second question waits
  `pending` until the first ends. States: `pending` (queued; `resumes_at` set while it waits for the LLM usage
  limit), `answering`, `answered`, `failed` with a plain `failure` sentence (job failure by typed cause, an answer
  that did not pass the checks, a cancellation naming its cause: the analysis was cancelled, the PR session was
  removed by a re-analysis, or another cancellation; a server error). A failed pipeline and Retry leave open
  questions running; a ready answer is not published once the analysis is cancelled; a failed question is asked again
  as a new one. An answer is shown only after its schema, the wording rules of brief §1, plain text (no markup, no
  link other than `https://github.com/…`), at most 8000 characters and 50 evidence lines inside their file of the
  pinned tree; otherwise the question fails and the answer is never shown. After a server restart every `pending`,
  `answering` or provisionally `answered` question (a ready output whose job result was not applied yet) is
  followed again to its end (a provisional answer returns to `answering` until the job's current attempt shows
  it again or its result arrives); only final answers enter later context packs. Logging: the provider's failure
  text appears only in the server's warn log, scrubbed of known secrets; no log line carries a secret, a token or
  the question text, and no response, event or stored question carries the provider's text. `GET …/questions`
  lists only the signed-in user’s questions of the analysis, oldest first, in any state. SSE applies the same filter.
  Each current HTTP/SSE response carries `asked_by` (GitHub login) and `mine` (always true for a current response). These fields
  are optional only for older payloads. There is no question edit/delete/retry endpoint; another POST is a new
  question owned by its sender. Plain text:
  HTML-like tags (`<` followed by a letter, `/` or `!`) are allowed only inside code spans and closed fenced
  blocks; a GitHub URL carries no user info and no explicit port.
- **MemCan status**: Settings and Tour PR-context facts expose `memcan_status {state, last_success_at}`,
  separate from `project_profile.memcan` (the stored analysis lookup). Read status never calls MemCan.
  `not_checked` means configured but no completed attempt since startup; `reachable` means the last attempt
  reached the service; `not_reachable` means connection failure/timeout; `not_configured` means no token
  (normal startup requires it under ADR-001). `last_success_at` is null before the first successful tool call,
  retained after failures, and reset on process restart. It is not a live health guarantee.
  Rule kinds include `lexer` for Rust comment-only lines, `header` for generated markers, and `binary`
  for git's binary detection, in addition to `path_glob` and `line_regex`.
- **Settings**: `repos` lists the tenant's repositories with at least one analysis; each opens
  `/api/v1/repos/{owner}/{name}/settings` (owner and name match case-insensitively; any other repository is
  `not_found`). Its rules, related repositories and profile source come from the newest stored pre-pass of the
  repository; without one the route answers `analysis_not_finished` ("Not known yet …"), never invented values.
  `llm_status` is the newest LLM fact: the outcome of the most recent call the runners reported, or a credential
  reported unusable after it (`not_working`, threat model T13); `null` before the first call. A failure's
  `message` is one fixed plain sentence chosen by the typed cause stored with the call
  (`llm_credentials.last_call_failure`, from the job outcome): usage limit reached; sign-in to the LLM provider
  needed; the provider rejected the request; the answer was not in the expected format; the call took too long;
  the call failed for another reason (also when no cause is stored). The provider's raw text stays in the
  database and the server log, never in the response. Installations are read from GitHub with the user token
  (`GET /user/installations`, suspended ones left out); when the user token is missing, GitHub rejects it or
  GitHub fails, `app_installations` is `null` ("not checked", no cause given; signing in again may help) and the
  rest of Settings is still returned, the session stays valid; `[]` means none installed. After a GitHub 401 the
  next request refreshes the user token; if GitHub rejects the refresh, the sign-in ends (later requests: 401). `PUT …/settings/language` accepts only `en` and `pl`.
- **Personal state indicator**: every current HTTP/SSE `AnalysisSummary` includes `has_my_state`, true iff
  the current reader has a stored `review_states` row (even an explicitly saved empty one). It never derives
  from another user's state or merely from the existence of the shared analysis. Older payloads may omit it.
- **Feedback calibration**: every current `Feedback` includes `calibration_example`, true only for
  `overrated`/`underrated`. “Saved to calibration memory” requires both true and `memory: written`;
  other written values are “Recorded”. Pending/failed retain their existing delivery meaning. The writer
  logs each confirmed write/delete at INFO with only the feedback id, never the note or memory text.
- **Review state**: `PUT` replaces the user's whole state (idempotent; repeated ticks are stored once); concurrent
  writes leave one of them whole. Every ticked step and the last viewed block must be in the analysis's Tour
  (quiet rows only when the Tour shows them), else `invalid_request` and nothing is stored.
- **Classic diff** (`diff`): unified hunks of the file that overlap the range on `side`, three context lines,
  hunks at most six unchanged lines apart merged (as `git diff -U3`); lines of `side` outside the range ±3 are
  left out, changed lines of the other side are shown whole, at most `RangeQuery::MAX_LINES` of them, else
  `range_too_large`. A file of the pinned tree outside the pull request diff has no hunks; a binary file is
  `not_rendered`. Line endings are kept: `text` has none, `DiffLine.eol` is `null` for LF, `crlf` for CR LF and
  `no_newline` for a last line without one, so a change of only the line ending shows as a removed and an added
  line with equal text and different `eol` (as git's `\r` and "\ No newline at end of file").
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
| `unavailable` | 503 | the database was busy or locked past its wait; carries `Retry-After: 1`. The web client retries a GET, PUT or DELETE once after that delay, never a POST |
| `insufficient_scope` | 403 | a valid access token without the route's scope |
| `token_not_accepted` | 403 | an access token on a route that is not `read` or on a non-safe method |
| `refresh_running` | 409 | dashboard: a refresh of the user is already scanning (R2) or live (R4) |
| `cursor_expired` | 409 | dashboard items: the cursor belongs to an older `revision`; read again from the start |
| `token_limit_reached` | 409 | the user has 10 live access tokens |
| `invalid_dashboard_settings` | 422 | dashboard settings or time zone break a rule of plan §F.6; the message names the field and the rule |
| `github_rate_limited` | 429 | starting a refresh while the user's GitHub rate limit is exhausted; `Retry-After` and the message give the reset time |
| `rate_limited` | 429 | over an access token's request rate (120/min), a failed attempt past a client's 30 failed bearer attempts/min, or a user's 17th open event stream (analysis and dashboard SSE, cookie or token); `Retry-After` |

## Endpoint × fixture-state inventory
Fixture PR `acme/node-client#482` (`fixtures/pr-482/`): `api-seed/default/` plus merge patches per state.
"–" = identical to `default`.

| Endpoint | default | wrong | missed | noq | noprofile | resume |
|---|---|---|---|---|---|---|
| `me` | signed in | – | – | – | – | – |
| `pr/resolve` | no analysis | – | – | – | – | existing finished analysis |
| `analyses/{id}` | finished, 5 flows | – | – | – | – | – |
| `tour` | 5 flows / 17 steps | step 3.1 rated high (confidently wrong) | – | no `flag` annotations, no assessments | `project_profile.sources = []` | – |
| `lines`, `diff` | ranges of #482 | – | `src/rpc/client.rs` 96 changed, no annotation | – | – | – |
| `review-state` | empty | – | – | – | – | ticks f2.1, last viewed step f1.5 |
| `questions` | none | – | – | – | – | one answered on f1.5 |
| `feedback` | none | `wrong` + `overrated` on step 3.1 | `missed_by_llm` line 96 | – | – | – |
| `pending-review` | empty | – | one LINE comment on line 96 | – | – | – |
| `settings` | working, en, 1 repo | – | – | – | – | – |

### Related-work targets, readable quiet rows and configuration failures

The agent's `RelatedWork` output schema is unchanged. `PrContextFacts.related_work_links` is an optional list
of server-projected targets associated with `llm.related_work` by exact `reference`. Each target contains
`repo` (`owner/name`), a positive safe-integer `number`, `kind` (`pull_request` or `issue`) and `verified`.
The server parses only whole GitHub references (`owner/name#number`, `#number`, or canonical HTTPS GitHub
pull/issue URLs). Arbitrary prose and other URLs produce no target. GitHub's issue endpoint confirms existence
and kind using the signed-in user's token; denied, missing or unavailable targets retain `verified: false`.
Verification is bounded to 20 targets and five seconds for the whole lookup. The old `related_work_checked`
field still describes the agent's GitHub access, not individual target verification. Clients build links only
from the structured fields, open a new tab with `rel="noopener noreferrer"`, and mark unchecked targets.

```json
{
  "related_work_links": [
    {"reference":"acme/node#42","repo":"acme/node","number":42,"kind":"issue","verified":true},
    {"reference":"acme/node#43","repo":"acme/node","number":43,"kind":"pull_request","verified":false}
  ]
}
```

`NotAnalysedItem.renderable` is an optional boolean computed from the pre-pass's excluded file sides. Mechanical
text can be shown; generated, binary and files recorded above the code reader's byte limit cannot. `rule_kind` optionally names the actual detector
(`path_glob`, `line_regex`, `lexer`, `header`, `binary`), independently of the provenance badge.

```json
{"file":"Cargo.lock","side":"head","ranges":[{"start_line":1,"end_line":2}],"badge":"classified_by_path","rule":"Cargo.lock","renderable":true,"rule_kind":"path_glob"}
```

`AnalysisState.failed.reason` optionally carries an `ApiErrorCode`; the additive `configuration` code means
administrator action is required. The same kind is carried in `JobFailureKind`. Missing required agent skills
produce the fixed public sentence below, never the runner's diagnostic text. The UI offers no Retry for this
reason. Existing failures without a reason retain their retry behavior.

```json
{"state":"failed","flow":null,"reason":"configuration","message":"The server is misconfigured: a required agent skill is missing. Ask the administrator."}
```

The web renders CI source counts independently (including skipped and neutral), only says "all passing" when
both sources were read and every reported result succeeded, and describes missing sources as "not checked".
It attributes shared questions with `asked_by`/`mine`, offers Ask again only for `mine: true`, and promises
personal progress on Home only for `has_my_state: true`. Test deletion counts include replacements and are
labelled "test lines changed or removed" alongside additions. MemCan connection status is separate from profile
availability. Written feedback says "Saved to calibration memory" only for `calibration_example: true`;
other written feedback says "Recorded".

### Whole-analysis waiting progress

`AnalysisSummary.progress` is optional for old responses and present on current HTTP/SSE snapshots:

```json
{
  "state": {"state": "planning", "stage": {"kind": "plan", "started_at": "2026-11-17T10:35:00Z", "part": {"index": 6, "total": 23}}},
  "progress": {
    "started_at": "2026-11-17T10:00:00Z",
    "planning": {"done": 5, "total": 23, "state": "current"},
    "pr_context": {"done": 0, "total": 1, "state": "pending"},
    "flows": {"done": 0, "total": null, "state": "pending"},
    "finishing": {"done": 0, "total": 1, "state": "pending"},
    "estimate": {"remaining_seconds": 7320, "basis": "this_analysis", "rough": true},
    "percent": 17
  }
}
```

Stage states are `pending`, `current`, `done`. Plan `part` is omitted for one batch. Its fixed total comes
from the server-written batch header in existing first-generation job prompts, excluding quoted input.
Accepted plan documents count completed parts; repair jobs do not add parts. `started_at` is analysis creation
(or the first job of the current retry round). A stage timer still starts at the runner's claim.

ETA sums remaining plan parts times this run's mean completed plan duration (fallback: recent plan seconds
per changed agent line), one recent mean PR-context duration if needed, and remaining flows times this run's
mean flow duration (fallback: recent mean). Before planning ends, flow time uses recent seconds per changed
agent line and is `rough`. History is at most the 30 newest review-ready analyses (not cancelled, accepted plan
and PR context; most never finish while flows run on request) in the same tenant, successful jobs lasting
1 second to 3 hours; a flow counts once its document is accepted, and flow seconds per changed agent line come
only from finished analyses. A stage counts its jobs of the newest round in which it succeeded
(a flow retry or Retry starts a new round). Completed generations of a stage (including repairs) contribute to its
one duration; this analysis supplies samples only once the stage's document is accepted. Missing data for any remaining kind, or an estimate over
7 days, yields null. Basis prefers `this_analysis`, otherwise `lines_only` before planning ends, else
`recent_runs`. Queue delay and future repair runs are unknown. Paused and terminal analyses have no ETA.

After planning, `percent` counts completed plan parts, PR context, flows and finishing out of their total.
Before planning ends it uses completed measured work / (completed measured work + estimated remaining time);
without an estimate it is null. It stays below 100 until finished. SSE publishes the same projection on stage
changes; the browser does not poll for estimates. A null percentage means an indeterminate bar.

There is no precise agent-start event in the runner protocol; claim time includes repository preparation.

Private Ask history used for re-forking is scoped to `(pull_request_id, user_id)`, never another reader's
questions or answers. Review-state and feedback routes also select by the authenticated user; removing a
foreign feedback id answers 404. Draft comment text, and a create request whose outcome is unknown (with its
idempotency key), stay in the browser (sessionStorage of the tab, keyed by user login and analysis) until the
comment is added, its text is cleared, or that user explicitly signs out; an expired session or a 401 keeps
them for the same user. The server never stores drafts. Published GitHub review comments are public PR data.

### Provider-reported usage

Analysis summaries (including Tour and analysis SSE events) carry nullable `usage`:

```json
{"usage":{"input_tokens":1200000,"cached_input_tokens":900000,"output_tokens":45000,"reasoning_output_tokens":null,"jobs":7}}
```

Counts sum retained `jobs.result.usage` reports with `finished_at` set, across all analysis rounds,
repair generations and Ask. Cached input is part of input, not an extra charge. `jobs` counts reports,
including a report of zero tokens; jobs without a report do not contribute. An analysis without reports
has `usage: null`. Earlier attempts whose result was replaced when a job was requeued cannot be recovered.
Separate reasoning usage is not in the stored job-result contract, so it is unknown (`null`), not zero.
These are provider reports, not billing totals. The UI hides the line without reports.
Existing job-completion broadcasts refresh progress and the Tour's usage after Ask; no additional polling.

Settings exposes tenant totals in `llm_usage`:

```json
{"llm_usage":{"today":{"input_tokens":1200000,"cached_input_tokens":900000,"output_tokens":45000,"reasoning_output_tokens":null,"jobs":7},"last_7_days":{"input_tokens":3400000,"cached_input_tokens":2900000,"output_tokens":120000,"reasoning_output_tokens":null,"jobs":23}}}
```

Windows use job completion time, from the server's local midnight through now; `last_7_days` includes
today and the previous six calendar days. Each window is null without reports. One aggregate SQL query
reads both windows; no counters or tables are added. Old responses may omit `llm_usage` and `usage`.

### Large pull requests before starting

Resolve returns `size_warning: null` below 3,000 additions plus deletions, or when GitHub did not provide
the size. Otherwise it returns:

```json
{"size_warning":{"changed_lines":27081,"changed_files":371,"estimate":{"remaining_seconds":14400,"basis":"lines_only","rough":true}}}
```

`changed_lines` and `changed_files` come from the same GitHub PR metadata request already used by resolve.
There is no additional GitHub call. `estimate` uses the same tenant-scoped recent-run samples and formula
as waiting progress for what starting the analysis runs, plan and PR context: historical plan seconds per
changed agent line × PR changed lines + mean PR-context seconds. Flows are analysed on request and are no part
of it. It uses `lines_only`, `rough: true`. PR metadata includes quiet lines too; this is
an intentionally rough comparison. Missing samples or an estimate above seven days produce null.

Independent of the threshold, resolve carries two optional fields from the same GitHub response (no extra
request): `size {changed_lines, changed_files}` for a pull request of any size (absent when GitHub did not
report it) and `planning_estimate {remaining_seconds, basis, rough}`, the same plan + PR context estimate
(absent without the size or enough history; `basis` is `this_analysis | recent_runs | lines_only`, before the
start always `lines_only`). `size_warning.estimate` equals `planning_estimate`.
The lookup never starts an analysis and makes no time claim without history; "Start analysis"
(`POST /api/v1/analyses`) does, and runs plan and PR context only.
Old resolve responses without `size_warning`, `size` or `planning_estimate` remain readable.

`analysis` in the resolve response is the tenant's latest analysis of the pull request whatever its language:
usable analyses (not cancelled; a failed one can be retried) before cancelled ones, then the user's current
language before another, then the newest. Every
`AnalysisSummary` carries `language` (`en` | `pl`), so a client can label it. Changing the language therefore
does not hide an existing analysis; `POST /api/v1/analyses` still creates an analysis per head and language
(409 only when a live one exists for this head and the user's language), so analysing again in the new
language stays an explicit action.

### Installation LLM choice and user pause

Settings includes optional `llm_selection`: model options `{id,label}`, allowed efforts, `current`
`{model,reasoning_effort}`, nullable `changed_by` login and `changed_at` UTC timestamp. PUT validates the
fixed catalog; invalid choices return `invalid_request`. Any signed-in allowlisted user can change it;
CSRF is required. The setting is read when each new job is submitted, including later jobs of a running
analysis. Existing job specs stay immutable. `AnalysisSummary.models_used` lists distinct choices in
first-use order from claimed jobs of all rounds (Ask included); empty means no job has started.
Settings `repos` is sorted by latest analysis time, then owner/name.

Pause returns `state: {state:"paused", reason:"user", paused_by:"alice", paused_at:"2026-11-17T18:04:00Z",
resumes_at:null, done:3, total:5}` (existing stage fields remain optional). It is idempotent, retaining the
original pauser and time. A running job finishes and its result is kept, but no next job is submitted or
claimed. The pause survives restarts. Provider pauses use `reason:"usage_limit"` (also the meaning of
an absent reason); a provider deadline never overrides a user pause. Resume clears the user pause;
if the provider is still limited (a paused job, or a waiting job whose credential another analysis paused),
its pause remains with its `resumes_at`. Resume checks the pinned head with GitHub and
returns `conflict` with the existing moved-head sentence if it changed. Neither action discards results or increments the retry round.
Once the plan and PR context are accepted, a paused analysis retains its partial Tour; Ask works on its accepted steps.
Cancel requires an inline UI confirmation, then discards documents and cancels unfinished jobs. Analytic and private working PR sessions are retained.

### Lazy flow review

`AnalysisSummary.review_ready` is true after accepted plan and PR context, including while running or
paused; false after Cancel. `background_analysis` is the stored switch; a new analysis starts with it `false`
(owner decision, 2026-11-18): after plan and PR context nothing is analysed until a flow is requested or the
switch is turned on. Reading the analysis, its Tour or a flow never requests work. The Tour now
contains every planned flow. Until accepted, `steps` and graph nodes are empty and `llm.risk` is null;
clients show `analysis_status`, not a risk label or a review tick. `planned` carries intent, rank and
verified changed ranges; the sum of `end_line - start_line + 1` over `planned.ranges[].ranges[]` is the flow's
size in changed lines, known before the flow is analysed (no time estimate per flow). Accepted flows support
ticks, comments and private Ask before the run ends.

Example: `{"analysis_status":{"state":"analysing","started_at":"2026-11-17T20:00:00Z","message":null},
"planned":{"intent":"Bound retries","importance_rank":2,"ranges":[{"file":"src/rpc.rs","side":"head","ranges":[{"start_line":8,"end_line":19}]}]}}`.
States: `not_analysed`, `queued`, `analysing`, `analysed`, `failed`. A claimed job's timestamp includes
repository preparation. Unaccepted planned ranges appear under their flow, not as accepted coverage.

POST `/api/v1/analyses/{id}/flows/{flow}/request` has no body and returns `AnalysisSummary`. Unknown flow
is 404. It is accepted while the analysis is planning, running or paused and review-ready, and when it is
`finished` (else 409 `conflict`). A flow whose stored document passes today's checks is left as it is (no
change). A stored flow that today's checks reject (other than for lines they newly require: those lines are
Unexplained and the flow stays analysed) is shown `failed` with `The stored analysis of this flow does not pass
the server's current checks, so its changes are listed as not analysed. Analyse this flow again.` (in a `failed`
analysis: `Retry the analysis; then analyse this flow again.`); the problem codes go to the server log only.
The Tour's button for it, and for any failed flow of a `finished` analysis, reads "Analyse this flow again" and
says before the click: "Replaces this flow's steps. Ticks, questions and feedback on them are removed."
Requesting it discards that document (only the version the server verified: a result stored since is kept) and
requests the flow in one transaction: it runs in a new job round like a failed flow (the rejected output is
never repaired), with the automatic retries of any requested flow. The same transaction removes what was said
about its steps, whose ids the new run reuses: every user's ticks of `<flow>.` steps, the questions on its steps
with their answers (their unfinished ask jobs are cancelled) and the feedback on its steps and annotations
(removed as by its author, so its MemCan memory is deleted). Comments and pending review comments stay: they are
anchored to code lines. Switching background analysis on ("Analyse all remaining flows") discards and requests
every such flow the same way, in the transaction that switches it on. In a `finished` analysis any `failed` flow
may be requested: the analysis becomes `running` again and finishes when its flows are analysed; while it runs
again `finished_at` is null, and `analysis statistics` logs the new finish with `refinish=true`, `finished_at` then
being the last finish. Cancel of such an analysis only stops that work: it is `finished` again with its results
and first finish time, and each flow left without a result is `failed` with `The new analysis of this flow was
cancelled. Analyse this flow again.`. Resume and Retry read a plan accepted earlier (whole, or the batch plans
of an analysis that was planning in batches) with the same stored-reading rule.
PUT `/api/v1/analyses/{id}/background` takes `{"enabled":false}` and returns `AnalysisSummary`.
Both require session/CSRF and tenant access; requesting work or enabling background checks the current PR
head. Latest requested flow runs first, then background flows by plan rank. Already claimed work finishes;
queued work may be withdrawn; a repair run refused because the demand changed waits for its flow's next turn
and then repairs the stored output, also after another flow's retry started a new job round. A user pause still
prevents submission/claim. A flow nobody requested never runs while background analysis is off. A failed flow
is retried automatically at most twice (per flow, whoever started it): a requested flow keeps its demand and
runs again at once (state `queued`), also with background analysis off; a flow analysed in the background is
retried after the flows not analysed yet, while background analysis is on. Then it stays `failed` and needs
explicit Retry, which runs it once more. Its `analysis_status.message` names the cause and what happens next:
`The flow analysis failed. <cause sentence of the failed call>` followed by `It will be tried again
automatically.` while the analysis is planning, running or paused and the server will run the flow again by
itself (it is requested, or an automatic retry is left and background analysis is on), in a `failed` analysis
(where a flow request answers 409) by `Retry the analysis; then this flow can be analysed again.` (with reason
`configuration`, which offers no Retry: `Ask the administrator to fix the server configuration.`), in a
`cancelled` one by `Start a new analysis to analyse this flow.`, else by `Retry this flow.` (likewise after `The flow output failed
verification after repairs.`). A flow too large for the agent
request (`This flow is too large for the agent request.`) is never retried automatically: the same plan and
diff give the same prompt. An unusable LLM credential fails the analysis instead (state `failed`
with the sign-in sentence and the flow). Requests/preferences survive restart.

When no flow work is admitted (every flow analysed or failed for good, or the rest not requested with background
off) and flows remain, the analysis stays `running {done, total, idle: true}` without `stage` and no server task
holds it; a flow request or switching background on starts the work again. `idle` (boolean, `false` when absent)
is read with every summary: `false` while a flow is requested, being analysed or waiting for its turn, so a
client can tell "nothing is being analysed" from "working between two jobs". `finished` still means every
planned flow is analysed. With background analysis off, `progress.estimate` and `progress.percent` cover plan
parts and PR context only (the wait until the review is ready) and are null once it is ready, until `finished`
(100); with a failed flow awaiting Retry the ETA is omitted. Unscheduled work has no completion time. Existing analysis SSE events invalidate the partial Tour,
including when a verified flow becomes available; no extra polling is needed.

#### Deterministic context and replacement lines

The lines response may include `context_only`, the changed line numbers on its returned side that the stored pre-pass covers by a line rule (Rust comments and plain private imports, brief amendment 2026-11-18); route and pre-pass use the same decision. Rust lines whose only change is whitespace are not in `changed` and not in a removed block: they read as unchanged code. These lines remain in the response and classic diff; the browser may hide them in the main view. A deleted block's optional `replacement` identifies the head range from the same diff hunk. Without it the block is a pure deletion. Example:

```json
{"context_only":[10,11],"deleted":[{"after_line":12,"base_start":13,"lines":["old();"],"total_lines":1,"replacement":{"start_line":13,"end_line":13}}]}
```

`DeletedBlock.context_only?: number[]` similarly names rule-covered base lines within a removed/replaced block. It does not remove their text from the response.

## Dashboard (v1.1)

Plan `docs/v1.1-dashboard-plan.md` §E (ADR-016, ADR-017, ADR-019, ADR-020). Types: `crates/contracts/src/api/dashboard.rs`
and `api/access_tokens.rs`; routes R1–R10 in `routes.rs`; both generated into `web/src/api/generated/` like every
HTTP type (the drift test covers them). Fixtures: `fixtures/dashboard/` (README there).

| ID | Method | Path | Auth | Request | Response |
|---|---|---|---|---|---|
| R1 | GET | `/api/v1/dashboard` | read | – | `DashboardSummary` |
| R1b | GET | `/api/v1/dashboard/items` | read | `ItemQuery` | `ItemPage`; weak `ETag`, `If-None-Match` → 304; 409 `cursor_expired` |
| R2 | POST | `/api/v1/dashboard/refreshes` | session_csrf | – | 201 `RefreshStatus` (supersedes a ranking, Refresh states); 409 `refresh_running` while one scans; 429 `github_rate_limited` |
| R2b | GET | `/api/v1/dashboard/refreshes/{id}` | read | – | `RefreshStatus` |
| R3 | POST | `/api/v1/dashboard/refreshes/{id}/cancel` | session_csrf | – | `RefreshStatus`; 409 `conflict` once the refresh ended |
| R4 | POST | `/api/v1/dashboard/refreshes/{id}/rank` | session_csrf | – | 202 `RefreshStatus` (Retry ranking of that snapshot); 409 `conflict` or `refresh_running` (Refresh states) |
| R5 | GET | `/api/v1/dashboard/refreshes/{id}/events` | read | – | SSE `DashboardEvent` (`sse-events.md`) |
| R6 | GET / PUT | `/api/v1/settings/dashboard` | read / session_csrf | – / `DashboardSettings` | `DashboardSettingsView` (PUT: 422 `invalid_dashboard_settings`) |
| R6b | PUT | `/api/v1/settings/dashboard/time-zone` | session_csrf | `SetTimeZoneRequest` | 204 (422 `invalid_dashboard_settings` for a zone `chrono-tz` does not know) |
| R7 | PUT | `/api/v1/settings/llm/judge-effort` | session_csrf | `SetJudgeEffortRequest` | `LlmSelection` |
| R8 | GET | `/api/v1/settings/access-tokens` | session | – | `AccessToken[]` |
| R9 | POST | `/api/v1/settings/access-tokens` | session_csrf | `CreateAccessToken` | 201 `CreatedAccessToken`; 409 `token_limit_reached` |
| R10 | DELETE | `/api/v1/settings/access-tokens/{id}` | session_csrf | – | 204 (404 for an unknown, revoked or another user's token) |

Every v1 `GET` route with session auth is `read` (ADR-019 lists them; paths and shapes unchanged); only R8 stays
`session`. How an agent reads with a token: `agent-api.md`.

### Rules
- **Scope.** Every response is the signed-in user's (or the token's user's) own: summary, items, refreshes, events
  and settings are keyed by user; another user's refresh id answers `not_found` (threat model T16). Private
  repository items appear only for the user whose token read them.
- **Revision and polling.** `DashboardSummary.revision` is opaque and changes whenever the stored snapshot or its
  ranking changes; `ItemPage.revision` names the revision a page belongs to. Agents poll R1 and read R1b only when
  `revision` changed. R1b also sends a weak `ETag` (hash of the body) and answers `If-None-Match` with 304.
  `Cache-Control: no-store` stays on every API response.
- **Items (R1b).** Rank order of the current snapshot. Filters, all optional: `tier`; `kind` = `pr` | `issue`
  (absent = pull requests and issues together; TODO items, plan §G, are listed whatever `kind` says while the user
  shows MemCan TODOs); `todos` = `false` leaves TODO items out (absent = `true`; a TODOs-only list is not supported
  in v1.1: filter `kind == "todo"` on the client); `repo` (`owner/name`); `state` (score state); `updated_since` (compared with
  `WorkItem.updated_at`). The web UI opens the list with `kind=pr` and offers PRs / issues / both (owner decision);
  the statistics (`SnapshotMeta.history`, headline, charts) count pull requests and reviews given only, with no
  switch (OD16). `limit` defaults to 100, at most 500
  (`ItemQuery::DEFAULT_LIMIT`, `MAX_LIMIT`; larger values are clamped); `cursor` is opaque, encodes the revision and
  position, and from an older revision answers 409 `cursor_expired` (the order changes when judgements land). No
  snapshot yet: an empty page.
- **Item keys** (normative; `ItemKey` in `api/dashboard.rs` is the only constructor and parser, and rejects any
  other form on deserialisation): `<owner>~<repo>~pr~<number>`, `<owner>~<repo>~issue~<number>` (GitHub owner
  `[A-Za-z0-9-]{1,39}`, repository `[A-Za-z0-9._-]{1,100}`, number without leading zeros, > 0) and
  `memcan~<uuid>` (lowercase hyphenated). URL-safe and stable across refreshes (`/dashboard/items/{key}` may be added
  later); stored judgements, `ScanNote.item` and `ItemText.also_keys` use it. The display alias `WorkItem.id` is
  not unique and never a reference.
- **Item shape.** `updated_at`: pull requests and issues = GitHub `updatedAt`; TODOs = when the dashboard first saw
  the current values of the fields of plan §G (MemCan has no `updated_at`). `llm` holds everything the LLM wrote:
  `why`, `next`, `alarm` and `also_keys` (items merged into this one, only from server-suggested pairs). `url` is
  built by the server only: GitHub `https://github.com/<owner>/<name>/pull|issues/<number>`, a TODO
  `<SR_MEMCAN_UI_URL>/tasks/<uuid>`, null without `SR_MEMCAN_UI_URL`. `review` is read from `analyses` at request
  time (newest analysis of the PR, head moved, the reader's saved state), never stored in the snapshot. `tags` and
  `facts` come from scan facts only; `llm` text is shown with the "LLM" tag.
- **Scores.** Integers only: points in tenths. `total_tenths` is the sum of the parts' `points_tenths` (a pending
  part counts 0); `parts` lists the twelve `ScoreComponent`s in display order, each with `source` and a plain
  `basis` (an estimate's basis names its rule, "estimate until judged: …"). `tier` is computed by the server:
  `p0` = LLM alarm, else `p1` ≥ 650, `p2` 350–<650, `p3` < 350 tenths, compared at full precision before rounding to
  tenths. `state`: `final`; `provisional` (an LLM part `inherited` after the item changed, or a computed default
  awaiting confirmation; shown `~N`, keeps its tier group); `unjudged` (new item, group "Not judged yet").
- **Refreshes.** One per user is `scanning` or `ranking` at a time. R2 and a scheduled start supersede a refresh in
  `ranking` and are refused only by one in `scanning` (R2: 409 `refresh_running`; schedule: counts as done), so a
  `paused` or `waiting_for_analysis` ranking never blocks a new scan. Transitions: "Refresh states" below. `failure.retry_at` is GitHub's
  rate-limit reset. `ranking` is null until the snapshot is stored; its `state` follows plan §B.3
  (`waiting_for_analysis` while a review analysis holds the LLM credential). `log` holds at most 200 plain lines
  without tokens or query strings. The newest 10 refreshes per user are kept, plus the one holding the current
  snapshot and every one started in the last 24 hours whose ranking judged items.
- **Schedule.** `ScheduleStatus.time` is the local `HH:MM`; `next_run_at` is null when off or without a time zone;
  `last_run.at` is the scheduled instant of that local date; `needs_sign_in` and `failed` are shown until a later
  refresh succeeds.
- **Settings (R6, R6b, R7).** An absent settings row means `DashboardSettings::default()` (plan §F.6). PUT
  validates every rule of §F.6 server-side (the client mirrors them) and answers 422 `invalid_dashboard_settings`
  naming the first failing field. `skip_authors` are excluded authors (every pull request and issue they author is left
  out of the dashboard, whatever its relation to the user; they are not bots); `known_bots` are the
  logins treated as bots besides GitHub `Bot` accounts and `[bot]` logins (absent in a request: the defaults,
  `thepastaclaw`, `Claudius-Maginificent`, `DCG-Claude`). R6b stores the browser's IANA zone (sent once on the first dashboard visit
  while none is stored). R7 sets the installation-wide ranking effort (`low` default, `medium`, `high`);
  `LlmSelection.judge_effort` reports it, and `changed_by`/`changed_at` are shared with the model choice.
- **Access tokens (R8–R10, ADR-019).** Token `srp_<id>_<secret>`: `id` 8 base62 characters, secret 32 random bytes
  base64url. Only `CreatedAccessToken.token` (R9's 201 body) ever carries the secret; no other response, log, event
  or error does. Name 1–60 characters, unique among the user's tokens that are not revoked; `scopes` = `["read"]`
  (anything else: 400 `invalid_request`); `expires_in_days` one of 30, 90, 365 (`CreateAccessToken::EXPIRY_DAYS`).
  A name, scope or expiry outside these rules answers 400 `invalid_request`, a name in use 409 `conflict`. At most
  10 live (not revoked, not expired) tokens per user. R8 lists tokens that are not revoked, newest first.
- **Token check** (middleware on the public router, WP11-TOKENS; only `Authorization: Bearer srp_…` is a token
  attempt, any other `Authorization` value passes to the session check): both a session cookie and such a token → 400
  `invalid_request`; a token on a route that is not `read` or with a non-safe method → 403 `token_not_accepted`;
  unknown, malformed, expired or revoked token, or its user off the allow-list → 401 `unauthenticated` with
  `WWW-Authenticate: Bearer` (no reason given); valid without the route's scope → 403 `insufficient_scope`; over the
  rate limits → 429 `rate_limited` with `Retry-After` (the token is checked first; only failures count against the
  per-address limit, IPv6 per /64). `GET /api/v1/me` answers a token with its user and `csrf_token: null`. Runner
  bearers sent to the public API reach the session check (401); `srp_` tokens are refused on the runner listener
  before any lookup (401). GitHub-backed reads (pending review, related-link checks, App installations) are reused
  for 60 s per user for tokens.
- **Refresh states** (the one state table; `RefreshState`, `RankingState` and `db::repo::dashboard` follow it).
  The current snapshot is that of the user's newest refresh in `ranking` or `finished`; `failed` and `cancelled`
  refreshes never hold one, so the previous snapshot stays. A stored snapshot stays current whatever happens to
  its ranking: the scan result remains visible with estimates for the items not judged.

  | From | Event | To | Ranking state |
  |---|---|---|---|
  | – | R2 or the schedule (none `scanning`) | `scanning` | – |
  | `scanning` | snapshot stored | `ranking` | `running` (then `waiting_for_analysis` / `running` / `paused`) |
  | `scanning` | scan fails | `failed` (`failure`) | – |
  | `scanning` | R3 cancel | `cancelled` | – |
  | `scanning` | server restart | `failed` (`interrupted`) | – |
  | `ranking` | ranking done (or nothing to judge) | `finished` | `finished` |
  | `ranking` | ranking fails | `finished` | `failed` (`message`) |
  | `ranking` | R3 cancel | `finished` | `cancelled` |
  | `ranking` | superseded by R2 or the schedule (same write transaction as the new `scanning` refresh) | `finished` | `cancelled` |
  | `ranking` | server restart | `finished` | `interrupted` |
  | `finished` with ranking `failed`, `interrupted` or `cancelled`, holding the current snapshot | R4 | `ranking` | `running` |

  A superseded refresh's snapshot stays current until the new scan stores its own. R2 while a refresh is
  `scanning` answers 409 `refresh_running`; a scheduled start then counts as done (`skipped`). R3 on an ended
  refresh (`finished`, `failed`, `cancelled`) answers 409 `conflict`. R4 answers 409 `conflict` for
  any other refresh or ranking outcome and 409 `refresh_running` while another refresh of the user is live. After a
  restart no refresh is live, so neither R2 nor the schedule is blocked. Each R4 starts a new ranking attempt
  (server-internal); a late result of an earlier attempt or of a superseded ranking is ignored.
- **Tolerant readers** (plan §E.5 E5-3): clients ignore unknown fields and treat unknown values of `kind`,
  `component`, `status`, `tier`, `TokenScope` and history `kind` as opaque. Changes stay additive (ADR-011).
