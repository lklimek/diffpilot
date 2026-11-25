# HTTP API (v1)

> v1.3 common amendment: [container contracts](containers.md) and [ADR-025](../decisions/ADR-025-container-runtime.md)
> govern payer/container identity, gateway boundaries, in-place 0030 cleanup and report 4.2. Runtime integration is
> owned by W3/W4/W5/W8; the older runtime description below applies until those packages replace it.


Source of truth: `crates/contracts/src/api/**` (types) and `crates/contracts/src/routes.rs` (`ROUTES`). Generated:
`web/src/api/generated/{types,routes}.ts`. This page fixes what the types cannot: authentication, CSRF, status
mapping, query rules and the endpoint × fixture-state inventory.

**Timestamps** (`format: date-time`, `contracts::time::Timestamp`): responses send RFC 3339 UTC with `Z` and the
fewest of 0, 3, 6 or 9 fraction digits. Requests (bodies and query parameters such as F6 `updated_since`) accept
strict RFC 3339 only: `YYYY-MM-DD`, `T`/`t`/space, `hh:mm:ss`, an optional fraction of 1–9 digits, `Z`/`z` or
`±hh:mm` (hours 00–23); a leap second `:60` reads as `:59`. Refused: RFC 9557 `[…]` annotations, the ISO 8601
basic format, missing seconds, `±hh`, `±hhmm` and sub-minute offsets, comma fractions, surrounding spaces and
`… UTC` (400 `invalid_request`).

## Listeners and authentication
- Public listener `SR_LISTEN_ADDR` (behind the reverse proxy): `/healthz`, `/auth/*`, `/api/v1/*`, the web UI.
- Runner listener `SR_RUNNER_LISTEN_ADDR` serves `/runner/v1/*` ([runner protocol](runner-protocol.md)).
- Webhook listener `SR_WEBHOOK_LISTEN_ADDR` defaults to `127.0.0.1:3002` and serves only `POST /webhooks/github`.
  The public listener drains requests to `/webhooks/*` before returning 404, up to 4 MiB and 8 seconds.
  A declared body over 4 MiB gets a 413 with `Connection: close` immediately. A drain that exceeds either limit
  returns 404 with `Connection: close`. The dedicated route registers `github_signature` authentication.
  Its contract-only handler returns 501. The receiver will authenticate the delivery by GitHub signature and
  App target, then check whether a tenant tracks the repository. [ADR-026](../decisions/ADR-026-github-webhooks.md)
- `session`: session cookie (`__Host-sr_session`, Secure, HttpOnly, SameSite=Lax, Path=/; `sr_session` without
  Secure only when `SR_PUBLIC_URL` is http and no trusted proxy forwards https), opaque token stored hashed. A
  session lasts 7 days, ends after 24 h unused, and only while its user stays on the tenant's allowlist (ADR-007
  addendum). `POST /auth/logout` ends this session (`everywhere`: all of the user's); the stored GitHub tokens end
  with the user's last session.
- `session_csrf`: session plus header `X-CSRF-Token` equal to `MeResponse.csrf_token`. Every non-GET public
  route with a cookie is `session_csrf` (contract test). CSRF applies to cookie-authenticated routes only; runner routes use
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
| GET | `/api/v1/analyses` | read | `MyAnalysesQuery` (`limit` 1–100, default 20) | `MyAnalyses`: the reader's own analyses (requested by them, this tenant), the latest per pull request, newest first; title, stored status and request time only (Recent reviews) |
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
| POST | `/api/v1/analyses/{id}/steps/{step_id}/questions` | session_csrf | `AskRequest` | 202 `AskQuestion` (anchor `step` without lines) |
| POST | `/api/v1/analyses/{id}/questions` | session_csrf | `AnchoredAskRequest` | 202 `AskQuestion` (R14 `askAnchored`, v1.2) |
| GET | `/api/v1/analyses/{id}/files` | read | – | `FilesResponse` (R12 `getFiles`, v1.2; 409 `analysis_not_finished` until plan and PR context are accepted) |
| PUT | `/api/v1/analyses/{id}/files/viewed` | session_csrf | `SetViewedRequest` | 204 (R13 `setFileViewed`, v1.2) |
| GET | `/api/v1/analyses/{id}/dependencies` | read | – | `DependencyView` (R15 `getDependencies`, v1.2; 409 `analysis_not_finished` until the analysis base is stored) |
| GET / POST | `/api/v1/analyses/{id}/feedback` | read / session_csrf | – / `FeedbackRequest` | `Feedback[]` / `Feedback` |
| DELETE | `…/feedback/{feedback_id}` | session_csrf | – | 204 |
| GET | `/api/v1/analyses/{id}/pending-review` | read | – | `PendingReview` |
| POST | `/api/v1/analyses/{id}/pending-review/comments` | session_csrf | `CreateCommentRequest` | 201 `CreateCommentResponse` |
| PATCH | `…/pending-review/comments/{comment_id}` | session_csrf | `UpdateCommentRequest` | `PendingComment` |
| DELETE | `…/pending-review/comments/{comment_id}` | session_csrf | – | 204 |
| GET | `/api/v1/settings` | read | – | `Settings` |
| PUT | `/api/v1/settings/language` | session_csrf | `SetLanguageRequest` | 204 |
| PUT | `/api/v1/settings/controlled-owners` | session_csrf | `SetControlledOwnersRequest` | `ControlledOwners` (R16 `setControlledOwners`, v1.2; 400 `invalid_request` for an owner that is not a GitHub login, more than 50 owners) |
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
  question runs as an `ask` job claimed before every other queued job (dashboard ranking, then batch; no pre-emption); one job runs per PR and credential, so a second question waits
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
- **Anchored Ask** (R14, v1.2). Same rules as the step route; the question is about `anchor`: `step` (a stored
  step of the Tour, `not_found` otherwise; optional `lines` inside the step's range) or `lines` (`path` of a file
  of the pull request on `side`; `range` within the file, or none for the whole file). Lines are 1-based,
  `start_line ≤ end_line`, at most `AskAnchor::MAX_LINES` (2000); anything else is `invalid_request`. The step
  route stores a `step` anchor without lines. Also `flow` (`flow_id` of a flow with a stored analysis, `not_found`
  otherwise; the flow must be shown analysed in the Tour, so a stored analysis that fails today's checks is
  `not_found` too) and `pr` (the whole pull request). `AskQuestion.anchor` is always set; `step_id` only for a step anchor.
  A lines question's job gets the lines, their classic diff (three lines of context, clipped), the owners of their
  changed lines from R12 and the overlapping steps in summary (chosen at submit and stored with the question); its
  context pack those steps in full. A flow question's job gets the flow's introduction and steps in summary, its
  pack the flow in full; a PR question's job gets the plan's flows and the PR context (each clipped), its pack the
  usual stored results. Packs are built at claim from stored documents only. Sessions as ADR-004.
  **Follow-ups** (v1.2): `parent_id` names a question of the same user on the same analysis (else `not_found`, as
  for a foreign question); the follow-up joins the thread of that question's first question (one level:
  `AskQuestion.parent_id` is always the first question) and must repeat its `anchor` (else `invalid_request`).
  Limits count follow-ups like any question. The job's prompt carries the thread's earlier questions of this user
  with their answers, oldest first: the first question and the latest follow-ups (at most 6), each answer clipped to
  4000 characters and each entry to 16 KiB (a clipped entry says how many characters are missing, and the block
  carries the clipped note), at most 32 KiB, with a note on omitted ones (ADR-004 addendum 2026-11-19).
- **Files** (R12, v1.2). Every file of the pull request in diff order: `path`, `old_path` (renames), `status`,
  `binary`, `added`/`removed`, `group` (`code`, or `not_analysed` with the path rule's category, badge and rule),
  `not_rendered` (`too_large`, `generated`, `binary`: the code of that file is never shown), `viewed`, and `hunks` (the diff
  without context: `base`/`head` ranges, `after_line` for a pure deletion). Each hunk's `parts` split its lines,
  base side first, into runs with one `owner`: `step` (the first step of the accepted flow whose verification
  covered the line), `flow` (a flow not analysed yet whose plan holds the line), `incidental` (the accepted flow's
  analysis names the line an incidental change off its control flow: `flow_id` and the LLM's one-sentence `text`),
  `unexplained` (with the Tour's reason), `not_analysed` (also inline test code a line rule recognised),
  `context_rule` (a comment or import line rule; context for the LLM only when its hunk also has other changes),
  `identical_move` (with both ranges) or `whitespace_only`;
  `also_in` lists the other steps whose lines include the run. Owners come from the coverage the Tour uses: the
  `unexplained` parts are exactly the Tour's Unexplained rows. `viewed` is the user's GitHub state (`viewed`,
  `unviewed`, `dismissed`: viewed before a later commit changed the file) read as the user;
  `viewed_status` says whether it was read (`checked`, `partial` beyond 3000 files, `not_checked` when GitHub did
  not answer; then every `viewed` is null, never assumed) and `current_head` is the head GitHub reported. An ended
  GitHub sign-in is 401 `unauthenticated` ("Sign in again"); a GitHub refusal of the read is logged at warn. An
  access-token reader gets the Viewed read cached for a minute.
- **Viewed** (R13, v1.2). `{path, viewed}` marks or unmarks one file of the pull request as the user on GitHub
  (`markFileAsViewed` / `unmarkFileAsViewed`). A path outside the pull request is `invalid_request`; a head that
  moved since the analysis is `pr_head_changed` (nothing is written; the head can still move between that check
  and the write); GitHub refusing it (an answer that proves nothing was applied, as for pending-review writes: e.g.
  the user token may not mark files, or a rate limit) is `github_refused`, nothing changed; a failed read of the
  pull request before the write is `github_unavailable` with nothing sent; any other answer to the write is
  `github_unavailable` (the mark may have changed; read again). One write per user and analysis runs at a time; a
  second one meanwhile is `conflict` before any GitHub call. Nothing is stored locally.
- **Dependencies** (R15, v1.2; ADR-021). The Cargo map of the head commit: `status` (`available`, or `not_rust`: no
  `Cargo.toml` at head, nothing checked), `controlled_owners` (the pull
  request's owner plus Settings, as applied to this response), `limits` (what was cut, not read or not resolved, with paths: `manifests_cut`,
  `manifest_unreadable`, `lock_unreadable`, `no_lock_file`, `workspace_unresolved`, `workspace_entries_cut`, `target_specific`,
  `source_unread`, `source_cut`, `facts_cut`, `definitions_unread`),
  `files_outside_crates` (changed files in no crate, not checked), `notes` (`produced`, `pending` before the plan is
  stored, `not_produced` for plans without notes) and `repositories`: this repository first, then controlled
  GitHub repositories, other GitHub repositories, other git URLs, registries (`crates.io`: the repository is not
  looked up), paths and unknown sources, holding the direct dependencies of the changed crates (at most 200 crates
  outside this repository and controlled repositories). Each repository lists crates: name, version,
  manifest path (workspace crates), the `Cargo.lock` or `Cargo.toml` line of its source, `used_by` (changed crates
  depending on it directly), `facts_about` (facts naming its types) and, for a changed crate, `changes`: changed
  lines and files, flows touching it, conversion `facts` (deterministic: kind, line, type, the type's crate, the
  import line, the definition line, `not_located`, `not_searched` or `other_repository`), `facts_hidden`, `other_facts` (facts about
  types of crates that are neither workspace members nor controlled, counted only), LLM `notes` (text, lines,
  changed lines; at most 3, `notes_hidden` counts the rest) and `other_dependencies` (direct dependencies over the
  view's cap, not listed). The map is computed once per analysis (older analyses at the first read) and stored; a
  mirror error, a failed definition search included, answers like the other analysis reads, nothing is stored and
  the next read computes it again. Definition files found but not read leave their facts `not_searched` with a
  `definitions_unread {dir, files}` limit.
- **Controlled owners** (R16, v1.2). Replaces the installation-wide list of controlled GitHub owners (case-insensitive,
  deduplicated, sorted); `Settings.controlled_owners` reports it with who changed it and when. The pull request's
  owner is controlled whether listed or not.
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
  next request refreshes the user token; if GitHub rejects the refresh, the sign-in ends (later requests: 401). `PUT …/settings/language` accepts only `en`, `pl` and `ru`.
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
| `analysis_not_finished` | 409 | tour, files, questions, review-state writes before `finished`; lines, diff before the analysis is ready (stored base is the merge base); repository settings without a stored pre-pass |
| `pr_head_changed` | 409 | comment write when the PR head moved (before or during the write); Viewed write when the head moved since the analysis |
| `comment_published` | 409 | edit or delete applied to a comment whose review was published meanwhile; or a new comment on a moved head removed from a review published meanwhile |
| `private_repository` | 422 | v1 is public repositories only |
| `app_not_installed` | 422 | comment routes without the App on the PR repo |
| `position_rejected` | 422 | GitHub silently rejected (`thread: null`) or target not commentable |
| `github_refused` | 422 | GitHub refused a change made as the user (the Viewed mark, R13: refused before executing it, a rate limit, or `FORBIDDEN`); nothing was changed |
| `not_rendered` | 422 | file classified as not rendered; diff of a binary file |
| `internal` | 500 | anything else (no detail) |
| `not_implemented` | 501 | a reserved route whose feature handler is pending |
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
`AnalysisSummary` carries `language` (`en` | `pl` | `ru`), so a client can label it. Changing the language therefore
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
and on the flow as a whole (with their follow-ups) with their answers (their unfinished ask jobs are cancelled) and the feedback on its steps and annotations
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

The lines response may include `context_only`, the changed line numbers on its returned side that the stored pre-pass covers by a comment or import line rule (Rust comments, plain imports, re-exports and multi-line `use` declarations, brief amendments 2026-11-18 and 2026-11-20; inline test code is not among them and stays shown); route and pre-pass use the same decision. Rust lines whose only change is whitespace are not in `changed` and not in a removed block: they read as unchanged code. These lines remain in the response and classic diff; the browser may hide them in the main view. A deleted block's optional `replacement` identifies the head range from the same diff hunk. Without it the block is a pure deletion. Example:

```json
{"context_only":[10,11],"deleted":[{"after_line":12,"base_start":13,"lines":["old();"],"total_lines":1,"replacement":{"start_line":13,"end_line":13}}]}
```

`DeletedBlock.context_only?: number[]` similarly names rule-covered base lines within a removed/replaced block. It does not remove their text from the response.

## Dashboard (v1.1)

Plan `docs/v1.1-dashboard-plan.md` §E (ADR-016, ADR-017, ADR-019, ADR-020). Types: `crates/contracts/src/api/dashboard.rs`
and `api/access_tokens.rs`; routes R1–R11 in `routes.rs`; both generated into `web/src/api/generated/` like every
HTTP type (the drift test covers them). Fixtures: `fixtures/dashboard/` (README there).

| ID | Method | Path | Auth | Request | Response |
|---|---|---|---|---|---|
| R1 | GET | `/api/v1/dashboard` | read | `SummaryQuery` | `DashboardSummary` |
| R1b | GET | `/api/v1/dashboard/items` | read | `ItemQuery` | `ItemPage`; weak `ETag`, `If-None-Match` → 304; 409 `cursor_expired` |
| R2 | POST | `/api/v1/dashboard/refreshes` | session_csrf | optional `StartRefreshRequest` | 201 `RefreshStatus` (supersedes a ranking, Refresh states); 409 `refresh_running` while one scans; 429 `github_rate_limited` |
| R2b | GET | `/api/v1/dashboard/refreshes/{id}` | read | – | `RefreshStatus` |
| R3 | POST | `/api/v1/dashboard/refreshes/{id}/cancel` | session_csrf | – | `RefreshStatus`; 409 `conflict` once the refresh ended |
| R4 | POST | `/api/v1/dashboard/refreshes/{id}/rank` | session_csrf | – | 202 `RefreshStatus` (Retry ranking of that snapshot); 409 `conflict` or `refresh_running` (Refresh states) |
| R5 | GET | `/api/v1/dashboard/refreshes/{id}/events` | read | – | SSE `DashboardEvent` (`sse-events.md`) |
| R6 | GET / PUT | `/api/v1/settings/dashboard` | read / session_csrf | – / `DashboardSettings` | `DashboardSettingsView` (PUT: 422 `invalid_dashboard_settings`) |
| R6b | PUT | `/api/v1/settings/dashboard/time-zone` | session_csrf | `SetTimeZoneRequest` | 204 (422 `invalid_dashboard_settings` for a name that is not an exact IANA name of the bundled tz database) |
| R7 | PUT | `/api/v1/settings/llm/judge-effort` | session_csrf | `SetJudgeEffortRequest` | `LlmSelection` |
| R8 | GET | `/api/v1/settings/access-tokens` | session | – | `AccessToken[]` |
| R9 | POST | `/api/v1/settings/access-tokens` | session_csrf | `CreateAccessToken` | 201 `CreatedAccessToken`; 409 `token_limit_reached` |
| R10 | DELETE | `/api/v1/settings/access-tokens/{id}` | session_csrf | – | 204 (404 for an unknown, revoked or another user's token) |
| R11 | POST | `/api/v1/dashboard/mentions/{id}/hide` | session_csrf | – | 204 (idempotent), 404 outside the user’s snapshot/hidden set |
| R12 | PUT | `/api/v1/dashboard/items/{key}/snooze` | session_csrf | `SnoozeRequest` | 204; 400 `invalid_request` for days outside 1–365 or a malformed key; 404 for an item outside the user's current snapshot |
| R13 | DELETE | `/api/v1/dashboard/items/{key}/snooze` | session_csrf | – | 204 (also when the item was not snoozed) |

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
  snapshot yet: an empty page. Pull request items carry `change_size` (plan §C.2 K16: `tiny` < 150, `small`
  150–499, `medium` 500–2999, `large` 3000–9999, `huge` ≥ 10 000 changed lines, with `additions`/`deletions` as
  evidence; absent for issues, TODOs and unknown totals; clients show an unknown class by its name). There is no
  size filter on R1b. A pull request of the user that waits on others (plan §C.2 K15) is not an
  item: it is listed in `SnapshotMeta.awaiting` only (`AwaitingItem.total_tenths` is null), never ranked, counted
  or excluded, so no tier filter hands it out as work; `waiting_on` names whom to ping and `reason` why
  (`ready_label`, `plugin` with the source's reason and evidence, `review_requested`). `SnapshotMeta.ready_to_merge`
  lists, by display id, the user's pull requests ready to merge at the scan (plan §C.2 K17: into the default branch
  or a release branch, every check passed except ignored ones, nothing left to answer, no ready label, not held by a
  current plug-in); they are ranked. `merge_state_pending` counts those of the user's pull requests Ready to merge
  could list whose merge state GitHub had not computed by the scan (the next refresh reads them again); null when the snapshot did not record merge states.
  A summary read in a browser session keeps My work open for 3 minutes while the v1.1 refresh schedule applies.
  See the v1.4 dashboard status additions below.
- **Excluded items (R1, B4).** `SnapshotMeta.exclusions` tells what the rules left out of the ranking, computed
  with the ranking at read time from the stored scan facts (so every stored snapshot has it; the web shows "not
  recorded for this snapshot" only when the field is absent): `total` (= `excluded`) and the non-zero `counts` per
  `ExclusionReason` (`excluded_author`, `outside_owned_areas`, `draft`, `judged_not_eligible`,
  `merged_into_another`, `snoozed`; clients treat an unknown value as "another rule"). `?excluded=true` adds `items`: at most
  200 (`EXCLUDED_ITEMS_LIMIT`) `ExcludedItem`s by reason, then display id (key, kind, display id, repository,
  number, URL, plain title, author, reason, `merged_into`, `snoozed_until` for `snoozed`); no LLM text, but `judged_not_eligible` and
  `merged_into_another` are LLM decisions (the web says "by the LLM"), the other reasons deterministic rules. Without the flag the summary carries the
  counts only (a few hundred bytes; the list can reach tens of KB). Same scope as the rest of the summary.
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
  time (newest analysis of the PR, head moved, the reader's saved state), never stored in the snapshot. `comments`
  (pull requests only) is read in a batch from the review-loop tables. `CommentTally.total` counts the current
  comments from the tenant's last successful read, excluding PR-level questions and items the read no longer
  produces. `CommentTally.needs_you` counts queue items whose status is Needs you, including qualifying
  PR-level questions. Status comments and unassessed unlinked comments are excluded from Needs you. The tally
  is absent before the first successful read or when a PR's saved review data cannot be read. `watched`
  (pull requests only, omitted when false) reports the reader's active watch (RW1). Like `review` both are part of
  the page's `ETag`. `tags` and
  `facts` come from scan facts only; `llm` text is shown with the "LLM" tag.
- **Plug-in notes (ADR-022).** `WorkItem.plugin_notes` and the PR page's `plugin_notes` carry external
  statements as `PluginNote { plugin, source, url, as_of, summary, details, facts }`. The shape is unchanged.
  Render every statement attributed to `source`, as plain text; only the server-built `url` is an evidence link.
  A summary or detail has at most 300 characters; a note has at most 64 details and 6 facts (100 characters per
  label/value), with control and invisible formatting characters removed. PR Hygiene keeps its compact next
  action in `summary`; `facts` and labelled `details` include readable stage/engine state, stage age at aggregate
  data time and its basis, blockers, review requests and approvers, objectors, flags and counts, author/areas/base/
  milestone/update time, latest dated review per reviewer, and up to five recent stage changes with observed and
  entry times. The per-PR response supplies history, stale status and its generation time; `as_of` is the aggregate
  response's generation time. History is cached until the aggregate PR record changes (time-only fields excluded);
  its failure leaves aggregate context available and says "Stage history: unavailable". The UI shows the full
  statement on hover or keyboard focus of its compact note, with keyboard scrolling for long statements.
- **Scores.** Integers only: points in tenths. `total_tenths` is the sum of the parts' `points_tenths` (a pending
  part counts 0); `parts` lists the twelve `ScoreComponent`s in display order, each with `source` and a plain
  `basis` (an estimate's basis names its rule, "estimate until judged: …"). `tier` is computed by the server:
  `p0` = LLM alarm, else `p1` ≥ 650, `p2` 350–<650, `p3` < 350 tenths, compared at full precision before rounding to
  tenths. `state`: `final`; `provisional` (an LLM part `inherited` after the item changed, or a computed default
  awaiting confirmation; shown `~N`, keeps its tier group); `unjudged` (new item, group "Not judged yet").
- **Refreshes.** One per user is `scanning` or `ranking` at a time. R2 and a scheduled start supersede a refresh in
  `ranking` and are refused only by one in `scanning` (R2: 409 `refresh_running`; schedule: counts as done), so a
  `paused` or `waiting_for_analysis` ranking never blocks a new scan. `trigger` is `manual` (R2), `scheduled`
  (ADR-020) or `automatic` (started while the user had My work open, plan §M; never while a refresh is live). Transitions: "Refresh states" below. `failure.retry_at` is GitHub's
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
  `thepastaclaw`, `Claudius-Maginificent`, `DCG-Claude`). `slack_names` is the user's Slack names table (G18, at
  most 100 rows): a GitHub login (unique, any case) and the text "Copy for Slack" writes instead of `@login` (1–80
  characters as typed, no control character, no white space at either end; absent in a request: an empty table).
  My work reads it with R6 only for Copy for Slack (plan K19). R6b stores the browser's IANA zone (sent once on the first dashboard visit
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

### Unanswered mentions
`GET /api/v1/dashboard` (`DashboardSummary`) and `/api/v1/dashboard/items` (`ItemPage`) include `mentions:
Mention[]`, oldest first. This is a separate list, unaffected by ranked-item filters, returned only on the first items page
(without a cursor); cursor pages return `[]`, available equally to sessions and read tokens. Fields: `id`, `item_key`, `kind` (`pr`/`issue`),
`repo`, `number`, `title`, `author`, `is_bot`, `created_at`, `text`, `url`. Text is untrusted plain text; `url`
is built by the server. `is_bot` uses the user's known bots plus GitHub Bot type / `[bot]`. An empty list means
no unanswered, unhidden mentions found in the snapshot. `Mention.partly_read` is true when any bounded
conversation window was truncated or its count unavailable; older snapshots default it to false.
Both responses include `mention_coverage`: `unchecked_items` counts real candidates missed by budget, deadline or read failure;
`unchecked_logins` names logins whose mention search was unavailable (limited fallback for mention-only issues);
`timed_out_logins` names PR searches that exceeded their request budget. Coverage is independent of filters
and present on every items page. The browser displays it in the collapsed summary, even without rows,
without parsing or duplicating scan notes. Older snapshots default to an empty list and zero/empty coverage.

`POST /api/v1/dashboard/mentions/{id}/hide` (R11) has no body. Session + CSRF only, no personal access tokens. Returns
204 (including repeated hides), 404 for an identity outside the user's snapshot/hidden set. A hide changes the
summary revision and items ETag, persists per user across devices/refreshes, and leaves ranking/judgement alone.
New posts on the same item have independent identities. Hides are not an update to GitHub.

`PUT /api/v1/dashboard/items/{key}/snooze` (R12, owner SN1) takes `SnoozeRequest {days}` (1 to
`MAX_SNOOZE_DAYS` = 365) and snoozes an item of the user's current snapshot (`{key}` is an `ItemKey`) from now
for that many whole days; a new snooze of the same item replaces the one that has not ended. `DELETE` on the same
path (R13, "Wake now") ends it. Session + CSRF only, no personal access tokens; per user, across devices; never an update to
GitHub, the snapshot, the ranking or a judgement. A snoozed item that the rules would rank leaves Needs you and is
listed under Excluded with reason `snoozed` and `snoozed_until`; another rule's exclusion wins, and a P0 alarm is
never snoozed. The snooze ends at `snoozed_until`, or earlier when the snapshot shows a review request to the user
on the pull request with a known request time after the snooze, or a mention of the user on the item (not hidden)
newer than the snooze; it is then marked ended when the next snapshot is stored (refresh or pull-request re-read),
so a wake lasts after the request or mention is gone. Ended snoozes are kept with their end and its reason (date,
review request, mention, woken by the user). Snoozes change the summary revision and items ETag. Waiting on
others and Ready to merge are not affected.

## Review loop (v1.3)

Plan `docs/review-workflow-plan.md` §6 (ADR-023). Types: `crates/contracts/src/api/review_loop.rs` (the module
`api::comments` already holds the pending review of ADR-006); agent output `agent/verify_comments.rs`; routes in
`routes.rs`, generated like every HTTP type. Path parameters: `{owner}/{name}/{number}` is the pull request (as
`/repos/{owner}/{name}/settings`), `{item}` an item's per-PR number `C<no>`, `{kind}` `ntfy` | `webhook`.
Fixtures: `fixtures/review-loop/` (spike S13-1).

| ID | Method | Path | Auth | Request | Response |
|---|---|---|---|---|---|
| RC1 | GET | `/api/v1/prs/{owner}/{name}/{number}` | read | – | `PrPage`; a known PR that GitHub could not be read for yet is still served (header from the stored PR, `state` and `base` empty, the failure in `comments.read.failure`) |
| RC2 | GET | `…/comments` | read | – | `CommentsOverview` |
| RC3 | POST | `…/comments/read` | session_csrf | – | 202 (429 `rate_limited` under 10 s) |
| RC4 | GET | `…/comments/items` | read | `ItemsQuery` | `CommentItem[]`; `first_seen_head` is the head of diffpilot's first read, `written` (`Written { round?, commit?, at }`) where GitHub says the comment was written, from the latest read: for a comment of a round's own review that round with its head and end; for any other its review's commit (absent for a PR comment) and the review's submission or the comment's creation (absent when that read gives no time, or for a read stored before it was kept) |
| RC5 | GET | `…/comments/items/{item}/context` | read | `ContextQuery` | `CodeContext` |
| RC6 | POST | `…/comments/items/{item}/assess` | session_csrf | – | 202; 409 `conflict` while a pass is live; 429 `AssessmentAdmissionFailure::AssessmentDailyLimit { resets_at }` at 80 admitted UTC daily passes; 409 `credential_missing` without a usable linked plan |
| C20 | POST | `…/comments/assess` | session_csrf | – | 202; requests a pass for the PR's comments under the same admission rules as RC6 |
| CA1 | GET | `/api/v1/settings/comment-assessment` | read | – | `AssessmentSettings`: primary/A-B configuration and remaining/reset quota |
| CA2 | PUT | `/api/v1/settings/comment-assessment` | session_csrf | `AssessmentSettingsUpdate` | `AssessmentSettings`; stale revision → 409; never schedules assessment; at most 100 distinct `owner/repo#number` selections |
| CA3 | GET | `…/comments/assessment-comparison` | read | optional `cutoff` (UTC timestamp) | Latest `AssessmentComparison` for this PR and payer, including exact-count summary, nullable verdict/proposal, owner revisions and method usage; 404 without a payer-owned pass; no GitHub fetch |
| CA4 | GET | `…/comments/sources` | read | – | `CommentSource[]`, including cleaned context and zero-point versions; no GitHub fetch |
| RC8 | POST | `…/comments/items/{item}/decisions` | session_csrf | `DecideRequest` | 201 `Decision`; 409 `item_changed`; 409 `conflict` when its `plan_digest` no longer matches the plan |
| RC9 | POST | `…/comments/decision-batches` | session_csrf | `DecisionBatchRequest` | 201 `Decision[]`; 400 `invalid_request` for a gate row under `bulk`/`group`, an item asked again or no longer read (`gone_at`) under `bulk`, or, under `table`, a batch that leaves out a waiting gate item; 409 `item_changed` naming the changed items; 409 `conflict` naming the rows whose `plan_digest` no longer matches the plan |
| RC10 | GET | `…/comments/log` | read | – | `CommentLog`; subscribers of the tenant's live tokens; a push target of another user's token shows only its kind and delivery state (`url`, `topic`, `secret_last4`, `last_error` null) |
| RC11 | GET | `/api/v1/comments/carry-overs` | read | `CarryOverQuery` | `CarryOverPage` |
| RC12 | POST | `…/comments/items/{item}/undo` | session_csrf | `ItemActionRequest` | `CommentItem` (asked again; the undone decision keeps `undone_at`); 409 `conflict` without a standing decision or once an effect is claimed; 409 `item_changed` |
| RC13 | POST | `…/comments/items/{item}/reopen` | session_csrf | `ItemActionRequest` | `CommentItem` (state `reopened`; effects not yet claimed are cancelled); 409 `conflict` without a standing decision; 409 `item_changed` |
| RC14 | POST | `…/comments/items/{item}/skip` | session_csrf | `ItemActionRequest` | 204; the item comes last in the caller's queue while it stays at that revision |
| RC15 | POST | `…/comments/items/{item}/gates` | session_csrf | `GateMarkRequest` | `CommentItem`; marks or clears the owner's mark of one gate (assessment gates stay); 409 `item_changed` |
| RC16 | POST | `…/comments/effects-preview` | session_csrf | `EffectsPreviewRequest` | `EffectsPreview[]` (the effects the rows would get, with a `plan_digest` for PR-level rows; nothing stored); 400 `invalid_request` as RC8/RC9 |
| RC17 | GET | `…/comments/decide-rest` | read | – | `DecideRest` (the confirmation list of "Decide the rest per recommendation"; gate items and items without a complete recommendation are left out with their reason) |
| RW1 | PUT / DELETE | `…/watch` | session_csrf | – | 204; 409 `watch_limit_reached` |
| RW2 | GET | `/api/v1/watches` | read | – | `WatchList` (with the reader's assessment passes of the last 24 h and the daily limit) |
| RW3 | GET | `/api/v1/events` | read | – | SSE `UserStreamEvent`: `user_event`, `heartbeat`, `llm_activity` (reserved in B1) (`sse-events.md`) |
| RN1 | GET | `/api/v1/settings/notifications` | session | – | `NotificationSettings` (secrets as `secret_last4` only) |
| RN2 | PUT / DELETE | `/api/v1/settings/notifications/channels/{kind}` | session_csrf | `PutChannelRequest` / – | `NotificationChannel` / 204; 422 `scheme_not_allowed`; 400 `invalid_request` for a URL with credentials, a loopback, link-local or reserved address or a denied private address, a bad ntfy topic or secret; 422 `secret_required` when a kept secret would follow a changed URL scheme, host or port; 409 `conflict` when the stored target changed while the request was checked; `secret` is a `SecretChange` (absent or `keep`, `replace`, `remove`) |
| RN3 | POST | `…/channels/{kind}/test` | session_csrf | – | `ChannelTestResult` (one attempt); 429 `rate_limited` with `Retry-After` when 30 messages went to the channel within the hour (nothing sent, health unchanged) |
| RF1 | GET | `/api/v1/decision-feed` | read | `FeedQuery` | `DecisionFeed`; `ETag` |
| RF1b | GET | `/api/v1/decision-feed/decisions/{id}` | read | – | `FeedDecision` (the `fetch` link of `decision_available` and of push messages) |
| RF2 | GET | `/api/v1/decision-feed/events` | read | `FeedQuery` | SSE `FeedEvent`; registers the subscription |
| RF3 | POST | `/api/v1/decision-feed/decisions/{id}/claim` | token: review_loop_execute | `ClaimRequest` | 201 `Claim`; 409 `claimed`; 422 `capability_missing`; 409 `conflict` when nothing is left for an agent |
| RF4 | POST | `/api/v1/decision-feed/effects/{id}/reports` | token: review_loop_execute | `EffectReportRequest` | 201 (no body); 409 `claimed` (another token's claim, or never claimed by this token), 409 `lease_expired` (this token's lease ended, also after the decision went back to the feed), 409 `conflict` (the effect already ended); 400 for `done` without `reference`, `failed` without `message` or `done` while the `after` effect is not done; a repeated idempotency key on the same effect is a no-op; a `failed` effect cancels the unclaimed effects waiting on it |
| RF5 | PUT / DELETE | `/api/v1/decision-feed/push-target` | token: review_loop_execute | `PutPushTargetRequest` / – | `PushTarget` / 204; the calling token's own target only; as RN2 for the URL, and 422 `secret_required` when a kept secret would follow a changed kind or URL origin, 409 `conflict` when the target changed meanwhile |

### Rules
- **Assessment:** RC1, RC2, RC3 and RC4 can refresh the stored GitHub read. They never start LLM work. Browser
  clients call `startCommentAssessment` (C20) on entry to Comments and after a manual GitHub refresh that found new
  items. Items in the live pass input show `running`; other eligible items waiting for a subsequent pass show
  `queued`. An item title uses the first sentence of the host report's description, or the validated recommendation
  reason from PromptV4 (its necessity reason when the recommendation has no public text). It uses at most
  90 characters including an ellipsis when shortened.
- **Who decides (T22).** Decisions, batches, re-reads, re-assessments, watches and channel settings need a session
  with CSRF; a token gets 403 `token_not_accepted`. `RouteAuth::TokenOnly(TokenScope::ReviewLoopExecute)` (named
  `token_review_loop_execute` in `routes.ts`) marks RF3–RF5: a personal access token with the scope
  `review_loop_execute`, never a session (401 `unauthenticated` with `WWW-Authenticate: Bearer` without a token),
  any method (ADR-019 amendment: a token may call a mutating route only when the route names its write scope). These are the only non-`GET` public routes that are not `session_csrf`
  (contract tests `execute_scope_is_only_on_feed_write_routes`, `mutating_browser_routes_require_csrf`).
- **Revisions.** Every `CommentItem` carries `revision`; a decision names `item_revision` and gets 409
  `item_changed` when the item changed meanwhile (a read, an assessment, another user's decision). Decisions are
  append-only: a new decision supersedes the current one and cancels its effects not yet claimed.
- **D8** is `reject` with `override: {type: "blocker_gate", gate}`; gate ids are `G-FUNDS`, `G-SECRET`,
  `G-CRYPTO`, `G-DATA`. The owner's `comment` is stored and sent to the executor, never posted; `reply_text` is
  what gets posted.
- **P3** (`dismiss_stale_reviews`): the dismissal message is `reason` (the default message when absent);
  `reply_text` is not read. Clients show the server's texts from RC16, never their own copies.
- **Plan digest.** RC8/RC9 plan every row inside the transaction that stores it. For a PR-level row (P1
  `merge_with_follow_ups`, P3 `dismiss_stale_reviews`) RC16 returns `plan_digest`, an opaque digest of every
  effect the row plans (kind, executor, order, targets, text) and of what the plan rests on (P1 the items it follows
  up and their revisions; P3 the reviews it dismisses, each still stale). A row sent
  with the `plan_digest` the owner confirmed is refused with 409 `conflict` when the plan computed at storing
  differs; the client previews again and asks the owner to confirm the new plan. A row without it is stored as
  planned then.
- **Claims.** A claim covers every open agent effect of a current decision, for 30 minutes, renewed by each report;
  only decisions whose open effects all fall within the claimer's capabilities can be claimed. A report on another
  token's claim answers 409 `claimed`; after the claimer's lease ended 409 `lease_expired` (also once the feed task
  returned the decision, until another token claims it). `released` returns the decision to the feed.
- **Secrets (T28).** Channel and push-target secrets are accepted in requests only (`SecretChange`: an absent
  field or `{"action": "keep"}` keeps the stored one, `{"action": "replace", "value": …}` (8–4096 characters)
  replaces it, `{"action": "remove"}` removes it); responses carry `secret_last4`. A stored secret is kept only while
  the kind and the URL's scheme, host and port stay; otherwise 422 `secret_required`. The browser notification toggle stays per browser (local storage) and is not part of RN1.
- **Outbound payloads** (contract types, plan §5.5): webhook `WebhookPayload` `{v, event, text, url, pr, at}`;
  agent push `AgentPushPayload` `{v, event, decision_id, pr, fetch}`, never reply text, reasons, comment bodies or
  secrets (contract test `push_payloads_have_a_fixed_shape`).
- **Text.** Comment bodies are GitHub text (untrusted, rendered as plain text, links only to
  `https://github.com/…`); assessment, purpose and code descriptions are LLM text; bot severity is the bot's claim.
- **Job contract.** `agent::verify_comments::VerifyCommentsOutput` (Codex strict subset) and
  `job::spec::PullRequestCommentsTarget` are defined; `JobKind::VerifyComments` and the `JobTarget` variant come
  with the runner path (WP13-JOB), because they change exhaustive matches in the server, runner and the `jobs`
  table's CHECK lists.

## v1.3 common DTO additions (W1)

W5 registers `GET /api/v1/prs/{owner}/{name}/{number}/comments/sources` with a `CommentSource[]` response,
including zero-point source versions. `CommentItem.point` identifies a grounded quote and its source;
`duplicate_of` preserves independent item identities. Assessments add `already_fixed`, nullable `pre_existing`,
`evidence_text` and attributed `recorded_decision`; absent code/fix-size stay nullable.

Common gateway `CredentialStatus` uses singleton `mode: chatgpt_plan`; `LinkCredential` accepts no secrets,
API keys or base URLs. Routes for `GET`/`DELETE /api/v1/settings/codex` and
`POST`/`GET /api/v1/settings/codex/link` use the signed-in user, never a caller-supplied payer.
GET status and link polling synchronize canonical gateway status to the secret-free credential repository.
POST requires session + CSRF and returns `DeviceLink`; DELETE requires session + CSRF and returns 204 after
explicit gateway unlink. Both GET routes require a session; status returns `CredentialStatus`, while link
polling returns the staged view (an existing usable credential remains usable during replacement).
Unsupported modes and extra secret/base-URL fields are rejected before contacting the gateway.
Both views include safe D13 telemetry: gateway link time and observed primary/secondary plan percentage/reset
windows, plus server-computed payer UTC today/month usage and claimed/running container identities. Missing
observations remain null. Link polling also returns the current public `challenge` for reload/navigation recovery;
a read never starts a device flow. See [the approved W6 amendments](containers.md#w6-approved-account-status-and-conditional-settings-amendments).

The server connects through `SR_GATEWAY_ADMIN_URL` and the dedicated credentials scope
`SR_GATEWAY_CREDENTIALS_TOKEN_FILE` (or `SR_GATEWAY_CREDENTIALS_TOKEN`, mutually exclusive). Missing gateway
configuration or failed administration returns a redacted 503, preserving stored status. Redirects and implicit
HTTP retries are disabled. Calls use a 15-second timeout and a 16-KiB response cap. Per-user synchronization
serializes route operations in one server process. Settings must keep polling to observe async linking and
quarantine; job results also mark unusable credentials. The dev/e2e stack uses only a loopback fake admin
service and the same authenticated Settings routes, with no release-build seeding path.


The W1 `api::assessment` contracts define `GET`/`PUT /api/v1/settings/comment-assessment` (PUT body:
`AssessmentSettingsUpdate { config, revision }`, GET: `AssessmentSettings` with its opaque config revision) and `GET …/comments/assessment-comparison`
(`AssessmentComparison`, payer/PR scoped). They carry frozen method references, actual owner revisions and
safe remaining/reset quota. `AssessmentAdmissionFailure` distinguishes daily limit (with `resets_at`) from
missing usable credentials. W5/W6 implement route access checks and orchestration; changing configuration
alone never schedules work. Conditional writes compare the opaque config revision inside `write_tx`; a stale
request returns 409 without replacing another tab's selections. Clients refresh and require an explicit retry.
Cards show only the frozen primary; secondary results are comparison-only.


## v1.4 dashboard, Comments and activity

The following routes return 501 `not_implemented` in B1 once any required session, token or CSRF checks pass.
The response and status in the table describe their final contract.

| Method | Path | Auth | Request | Response |
|---|---|---|---|---|
| POST | `/webhooks/github` | github_signature | GitHub delivery | 202 when accepted, 200 for ping or a duplicate |
| POST | `/api/v1/dashboard/items/{key}/rejudge` | session_csrf | – | 202, 404 outside the current snapshot, 409 `conflict` unless outdated or when the daily cap is reached |
| GET | `/api/v1/settings/repository-installations` | read | – | `RepositoryInstallations` |
| GET | `/api/v1/settings/github-webhook` | read | – | `GithubWebhook` |
| GET | `/api/v1/llm/activity` | read | – | `LlmActivity` |
| GET | `/api/v1/settings/ranking-usage` | read | – | `RankingUsage` |
| POST | `/api/v1/settings/ranking-usage/reset` | session_csrf | – | `RankingUsage` |
| GET | `/api/v1/me/prs` | read | `PrHistoryQuery` | `PrHistory` |

- `WorkItem.judgement` describes current, outdated, queued, judging, not judged and capped items. An outdated
  judgement can name changed facts or a changed prompt. `DashboardSummary.status_counts` counts outdated
  (including judging), never judged (including queued and capped), not live and excluded items. The legacy
  `ItemText.inherited` and `older_prompt_judgements` fields remain readable. The optional R2 request defaults
  `rejudge_outdated` to false. B1 accepts the request with its existing refresh behaviour.
- `WorkItem.live` and the summary's repository counts and `last_event_at` describe live webhook coverage.
  `SnapshotMeta` omits `facts_checked_at`. Repository installations come from the database and cover repositories
  tracked by the reader's tenant through owners to list, repository rules, watches, analyses or current snapshots.
  Webhook settings show secret configuration, the optional public URL, the last verified delivery and the
  installation-wide 14-day count of verified deliveries dropped for untracked repositories.
- Fact tags may include stable filter keys and labels. Work items may include stack position, size, dependent PR
  numbers and a matching release base. `Settings.llm_usage.this_month` uses the server-local month. The ranking
  usage reset starts counting from the reset time. `resets_at` is the next midnight in the reader's configured zone.
- The activity response shows the reader's own jobs with optional subject, times and typed wait reason, plus
  other running or queued jobs in the same tenant with kind and a response-local opaque id. The pause and last
  call are the reader's own.
- `AssessmentState::Queued` waits for the next pass. `Running` identifies items in the live pass. Progress has
  `in_pass`, `waiting` and optional `since`. Comments can carry a reader's watch and items can carry a short
  LLM title, status-comment flag, imported-decision evidence, an import proposal or a primary-following flag.
  `Decision` carries imported evidence or a primary item number with its corresponding `via`. A proposal still
  needs the owner to decide it. See [the Phase B rules](../v1.4-phase-b-plan.md#6-needs-you-c10-and-duplicates-c15).
- `needs_you` counts queue items whose status is Needs you. `needs_you_gates` counts the gated subset. Running
  assessments and unassessed unlinked comments are excluded, with the exceptions in the [N1 rules](../v1.4-phase-b-plan.md#6-needs-you-c10-and-duplicates-c15).
  The PR badge and dashboard use this calculation. An open non-bot review item in the decision queue vetoes
  Ready to merge while awaiting assessment or included in a running pass. Status comments are listed and excluded
  from the queue, including `undecided`, and from `assessment.total`. Assess again on a status comment returns
  409 `conflict`. Ask can
  still use it as context. `imported_unseen` is omitted until the server tracks imports since the last Comments
  visit.
- The Tour's optional `StepFacts.leading_context` is the deterministic line range of doc comments and attributes
  immediately above the step's first item. The generic technology plugin leaves it absent.
- PR history merges the reader's requested analyses, watches including ended ones, and PR page visits. It returns
  one item per PR, newest activity first, in `prs` with `next_cursor` for pagination. A PR page visit updates the
  reader's `last_opened_at` when the history feature is active.

B1 omits uncomputed response fields. Explicit zero counts carry data once the feature handlers are implemented.
