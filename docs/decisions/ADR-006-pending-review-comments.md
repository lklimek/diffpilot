# ADR-006: Comments through the GitHub pending review (GraphQL)

- Status: accepted (coordinator, 2026-11-16, after spike S3; amended 2026-11-16 after the WP1-A contract review)
- Source: `docs/spikes/S3-pending-review.md`; brief §2 Comments, §5 Comments.

## Decisions
- **Writes** only with GraphQL: `addPullRequestReviewThread(pullRequestId, …)` (creates or reuses the user's
  pending review), `updatePullRequestReviewComment`, `deletePullRequestReviewComment`. REST is not used for
  pending reviews; the server never submits a review.
- **Serialised per (user, PR)** in the server: concurrent first comments create several pending reviews (S3 run
  80). Listing merges every pending review of the user.
- **Head check.** Before each write the server compares the PR's `headRefOid` with the analysis head; a mismatch
  is `pr_head_changed` ("PR changed, re-analyse").
- **LINE, FILE or not commentable** is decided by the server from the PR diff, not by GitHub, and published as a
  deterministic fact with the displayed lines (`LinesResponse.comments: CommentScope`):
  - every line of the target inside one diff hunk → LINE thread (side `head` = RIGHT, `base` = LEFT);
  - another line of a file that is in the PR diff → FILE thread (`subjectType: FILE`) whose body ends with the
    permalink `https://github.com/<o>/<r>/blob/<head>/<path>#L<a>[-L<b>]`;
  - a line of a file that is not in the PR diff (impact references, context in an unchanged file) → not
    commentable: `CommentScope::NotInDiff`; the UI offers the permalink to copy; a create request for such a
    target is rejected with `position_rejected` without calling GitHub.
  `thread: null` in a response (silent rejection) is `position_rejected`.
  For the owner: [deviation] brief §2 and §4.3 allow a comment on ANY displayed line; GitHub returns
  `thread: null` for a FILE thread on a file outside the PR diff (S3 run 22, finding 2) and no measured working
  variant exists, so in v1 lines of files outside the PR diff are not commentable (the permalink is offered
  instead).
- **Error kinds** in the API: `pr_head_changed`, `position_rejected`, `app_not_installed`, `not_found` (deleted
  elsewhere).
- **Idempotency.** Every create carries a client-generated `idempotency_key`; the server stores the request hash
  and response per (user, key); a repeat returns the stored response.
- **Listing** reads `reviews(states:[PENDING]){comments}` plus `reviewThreads` for sides; tolerant of the
  eventual consistency of `reviewThreads` after deletes.
- A comment on a changed line without annotation also records feedback `missed_by_llm`.
