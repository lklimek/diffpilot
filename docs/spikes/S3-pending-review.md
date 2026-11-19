# S3 — Pending review through the GitHub GraphQL API

Date: 2026-11-16. Target: P-REV `dashpay/dash-evo-tool#1036` (OPEN, head `fa12dd0a`). Token: O4 user-to-server
token of `Claudius-Maginificent` (`ghu_`, App `claudbot`, installed on the repository). API version header
`2026-03-10`. Only PENDING content was created; nothing was submitted; everything was deleted (see Cleanup).

## Question
Can the server keep the user's comments only in the user's GitHub pending review: create with the first
comment, add, multi-line, deleted line, edit, delete, find and reuse, file-level comment for a line outside the
diff, list?

## Runs
Scripts: `scripts/spike-lib.sh` (recording, sanitising), `scripts/queries.sh` (all GraphQL documents),
`scripts/s3-cleanup.sh` (delete + verify), `scripts/patch-lines.py` (diff line numbers per side).
Recorded responses (fake-github fixtures): `responses/NN-*.json` = `{request, status, headers, response}`.
All under `/data/artifacts/diffpilot/2026-11-16/spikes/S3/`.

| # | Call | Result |
|---|---|---|
| 01 | `reviews(states:[PENDING])` before start | none — nothing pre-existing to preserve |
| 10 | `addPullRequestReviewThread{pullRequestId, path, line, side:RIGHT}`, no review yet | **creates the pending review implicitly**; thread + comment `PENDING` |
| 11 | same with `pullRequestId` while a pending review exists, `startLine..line` RIGHT | **attaches to the existing pending review** (multi-line OK) |
| 12 | `side:LEFT` on a deleted line | OK, `diffSide: LEFT` |
| 13 | LEFT..LEFT range on deleted lines | OK |
| 14 | `startSide:LEFT` → `side:RIGHT` range | OK |
| 15 | `pullRequestReviewId` instead of `pullRequestId` | OK (same review) |
| 21 | RIGHT range spanning two hunks (gap outside the diff) | **accepted** |
| 20 | LINE thread on a line outside the diff | `{"thread": null}`, **no `errors`**, nothing created |
| 22 | FILE thread on a file not in the PR | `{"thread": null}`, no `errors` |
| 23 | `addPullRequestReview` while a pending review exists | `UNPROCESSABLE`: "User can only have one pending review per pull request" |
| 16, 17 | `subjectType: FILE` thread (body links `blob/<head>/path#L120[-L124]`), twice on one file | OK; two separate FILE threads; thread `line: 1`, comment `line: null` |
| 30 | list: `pullRequest.reviews(states:[PENDING]){comments}` + `reviewThreads` | all 9 comments; side only on the thread |
| 31/32 | REST `GET pulls/1036/reviews` user token / unauthenticated | PENDING visible to the author only |
| 33 | REST `GET reviews/{id}/comments` | `line`, `side`, `subject_type` all `null` for pending — **unusable for listing** |
| 40 | `updatePullRequestReviewComment` | OK; `lastEditedAt` stays null while pending |
| 41 | `deletePullRequestReviewComment` | OK; the thread disappears with its only comment |
| 42/43 | delete/edit an already deleted comment | `NOT_FOUND` "Could not resolve to a node …" |
| 45/46 | delete every comment one by one | **review disappears with its last comment** |
| 50/51 | `addPullRequestReview{commitOID, threads:[2]}`, then `deletePullRequestReview` | OK |
| 52–54 | `addPullRequestReview` with no threads/body | empty pending review exists (count 1, 0 comments); deleted |
| 60/61 | review with `commitOID` = older PR commit; then `addPullRequestReviewThread` | review keeps the old commit; the new thread's comment is on the **current head** |
| 70 | REST `POST pulls/1036/reviews` (no `event`) line outside diff | 422 "Line could not be resolved" |
| 71 | REST create review with `subject_type: file` | 422 — REST reviews endpoint has no file-level drafts |
| 80 | 3 concurrent `addPullRequestReviewThread{pullRequestId}`, no review yet | **3 separate pending reviews** for one user and PR |
| 63/64 | listing right after `deletePullRequestReview` | `reviewThreads` still showed 2 PENDING threads (review null) for ~3 s |

## Findings
1. GraphQL covers every item. Minimal write API: `addPullRequestReviewThread` with `pullRequestId` (creates or
   reuses the pending review), `updatePullRequestReviewComment`, `deletePullRequestReviewComment`,
   `deletePullRequestReview`. `addPullRequestReview` is not needed (only for pinning `commitOID`).
2. File-level pending comments work: `subjectType: FILE` + `path`; no line. Only for files in the PR diff. REST
   has no pending file-level path, so the S3 fallback ("post as review body") is not needed.
3. Invalid positions (line outside diff, file not in PR) fail **silently**: `thread: null` without `errors`. The
   server must treat `thread == null` as an error. Ranges spanning hunks are accepted, so the server decides
   LINE vs FILE itself from the diff it already has (every line of the range inside one hunk → LINE, else FILE
   with a link), instead of relying on GitHub to reject.
4. Thread lines refer to the **current PR head** diff. If the analysed head ≠ current head, line numbers can
   point to different code. The server compares `headRefOid` with the analysis head before writing.
5. One-pending-review is not enforced under concurrency (run 80). Writes must be serialised per
   (user, PR) in the server; listing must tolerate several pending reviews (merge them).
6. Deleting the last comment deletes the review; an empty review only exists if created explicitly.
7. `reviewThreads` is eventually consistent after deletes (seconds). Listing for the UI uses
   `reviews(states:[PENDING]){comments}` (consistent in all runs) plus `reviewThreads` for `diffSide`/`startDiffSide`;
   verification loops retry.
8. Pending content is visible only to its author (unauthenticated listing showed no PENDING).
9. Field quirks for the contract: comment `startLine` is `null` for single-line, thread `startLine == line`;
   comment has no side, thread has `diffSide`/`startDiffSide`; FILE threads report `line: 1`, comments `null`;
   `lastEditedAt` is not set while pending. Pagination needed beyond 100 threads/comments.
10. User token write requires the App installed on the repository (S4: `FORBIDDEN` on `dashpay/platform`).

## Decision (proposed ADR text)
> **ADR: Pending-review comments via GraphQL.** The server writes comments only with
> `addPullRequestReviewThread(pullRequestId, …)`, edits with `updatePullRequestReviewComment` and deletes with
> `deletePullRequestReviewComment`; the user submits on GitHub. Writes are serialised per (user, PR). Before a
> write the server checks the PR head equals the analysis head (else: refuse with "PR changed, re-analyse" —
> owner/coordinator may choose line mapping later). The server classifies a target as LINE (all lines in one
> hunk of the head diff, side from the diff) or FILE (`subjectType: FILE`, body ends with a permalink
> `https://github.com/<o>/<r>/blob/<head>/<path>#L<a>[-L<b>]`); `thread: null` is an error. The pending list is
> read from GitHub on demand (`reviews(states:[PENDING])` + `reviewThreads`), merging several pending reviews.
> REST is not used for pending reviews.

## Plan impact
- Contracts (review/comment API): comment target = `{path, side: LEFT|RIGHT, start_line?, line}` from the UI;
  response carries GitHub node ids (`PRRC_…` comment, `PRRT_…` thread, `PRR_…` review) and `kind: line|file`;
  errors: `pr_head_changed`, `position_rejected`, `app_not_installed`, `not_found` (deleted elsewhere).
- Server: per-(user, PR) mutex; head check needs `headRefOid` (one cheap query) per write.
- fake-github (`tools/`): replay `responses/1*`–`8*`, including the silent `thread: null`, the
  one-pending-review error, review vanishing with its last comment and the concurrency duplicate.
- WP3 tests: wording "pending" only; never call `submitPullRequestReview`/REST `event`.

## Cleanup
`s3-cleanup.sh` after every block; final listing (`99-final-verify.json`): 0 pending reviews and 0 pending
threads of `Claudius-Maginificent` on `dash-evo-tool#1036` (and `platform#5528`, S4). No review comments were
created on the PR after 16:00 UTC; the user's 5 published reviews date from October.
