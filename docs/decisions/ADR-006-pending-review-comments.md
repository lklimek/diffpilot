# ADR-006: Comments through the GitHub pending review (GraphQL)

- Status: accepted (coordinator, 2026-11-16, after spike S3)
- Source: `docs/spikes/S3-pending-review.md`; brief §2 Comments, §5 Comments.

## Decisions
- **Writes** only with GraphQL: `addPullRequestReviewThread(pullRequestId, …)` (creates or reuses the user's
  pending review), `updatePullRequestReviewComment`, `deletePullRequestReviewComment`. REST is not used for
  pending reviews; the server never submits a review.
- **Serialised per (user, PR)** in the server: concurrent first comments create several pending reviews (S3 run
  80). Listing merges every pending review of the user.
- **Head check.** Before each write the server compares the PR's `headRefOid` with the analysis head; a mismatch
  is `pr_head_changed` ("PR changed, re-analyse").
- **LINE or FILE** is decided by the server from the head diff, not by GitHub: every line of the target inside one
  hunk → LINE thread with the side from the diff; otherwise a FILE thread (`subjectType: FILE`) whose body ends
  with the permalink `https://github.com/<o>/<r>/blob/<head>/<path>#L<a>[-L<b>]`. `thread: null` in a response
  (silent rejection) is `position_rejected`.
- **Error kinds** in the API: `pr_head_changed`, `position_rejected`, `app_not_installed`, `not_found` (deleted
  elsewhere).
- **Idempotency.** Every create carries a client-generated `idempotency_key`; the server stores the request hash
  and response per (user, key); a repeat returns the stored response.
- **Listing** reads `reviews(states:[PENDING]){comments}` plus `reviewThreads` for sides; tolerant of the
  eventual consistency of `reviewThreads` after deletes.
- A comment on a changed line without annotation also records feedback `missed_by_llm`.
