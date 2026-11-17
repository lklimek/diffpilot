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
| DELETE | `…/feedback/{feedback_id}` | session_csrf | – | 204 |
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
  `cancelled`, unfinished jobs cancelled, stored results and the agent session discarded. Retry applies to a
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
  without annotation) and rejected from clients (`invalid_request`, it is not a `UserFeedbackValue`). A step or
  annotation target must exist in the analysis's flows under its canonical Tour id (`f3.1`, `f3.1.a2`; aliases
  such as `f3.01` are rejected), a line target must be a changed line of the analysed diff on its side (added on
  `head`, removed on `base`), the note at most `FeedbackRequest::MAX_NOTE_CHARS` (2000) characters, else
  `invalid_request`; a blank note is stored as none. Feedback is stored locally at once and written to MemCan
  asynchronously; `Feedback.memory` is the state of the current revision's copy there (`pending`, `written` =
  MemCan confirmed it and no earlier write of unknown outcome can still land, `failed` = the last attempt failed,
  retried on a back-off). Only `written` may be shown as "saved to calibration memory".
- **Ask** (`…/steps/{step_id}/questions`): only for a `finished` analysis (else `analysis_not_finished`) that no
  re-analysis replaced (else `conflict`); `step_id` is a canonical step id of the analysis's Tour (`f1.5`; aliases
  such as `f1.05` and unknown steps are `not_found`); `text` is trimmed, 1 to `AskRequest::MAX_TEXT_CHARS` (2000)
  characters, else `invalid_request`, and stored and shown as text. No idempotency key: each request is a new
  question. Limits (`conflict` with a plain message): 100 questions per analysis, 30 per user per hour. The
  question runs as an `ask` job claimed before batch jobs; one job runs per analysis, so a second question waits
  `pending` until the first ends. States: `pending` (queued; `resumes_at` set while it waits for the LLM usage
  limit), `answering`, `answered`, `failed` with a plain `failure` sentence (job failure by typed cause, an answer
  that did not pass the checks, a re-analysis that cancelled it, a server error); a failed question is asked again
  as a new one. An answer is shown only after its schema, the wording rules of brief §1, plain text (no markup, no
  link other than `https://github.com/…`), at most 8000 characters and 50 evidence lines inside their file of the
  pinned tree; otherwise the question fails and the answer is never shown. After a server restart every `pending`,
  `answering` or provisionally `answered` question (a ready output whose job result was not applied yet) is
  followed again to its end (a provisional answer returns to `answering` until the job's current attempt shows
  it again or its result arrives); only final answers enter later context packs. Logging: the provider's failure
  text appears only in the server's warn log, scrubbed of known secrets; no log line carries a secret, a token or
  the question text, and no response, event or stored question carries the provider's text. `GET …/questions`
  lists every question of the analysis, oldest first, in any state and of every user of the tenant.
  Each current HTTP/SSE response carries `asked_by` (GitHub login) and `mine` (reader is asker). These fields
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
