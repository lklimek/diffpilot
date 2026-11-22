# S13-2: GraphQL cost of the review-loop reads (Q-L1–Q-L4) and the Watch probe

Date 2026-11-22. Read-only `gh api graphql` with the host user token, cost from `rateLimit { cost }` of each
request. Queries as recorded in `fixtures/review-loop/github/` (Q-L2: threads `first:100`, comments `first:50`;
Q-L3, Q-L4: `first:100`; all with `pageInfo`).

## 1. Full read per PR

| ID | PR | Threads / reviews / comments | Pages Q-L1 + Q-L2 + Q-L3 + Q-L4 | Points | Largest answer | Slowest page |
|---|---|---|---|---|---|---|
| C1 | dashpay/platform#3133 (most threads found) | 235 / 98 / 36 | 1 + 3 + 1 + 1 | 6 | 482 KB (reviews) | 3.1 s (threads) |
| C2 | dashpay/platform#3841 (most reviews found) | 106 / 233 / 16 | 1 + 2 + 3 + 1 | 7 | 926 KB (reviews page 2) | 3.3 s (threads) |
| C3 | dashpay/dash-evo-tool#901 | 137 / 128 / 43 | 1 + 2 + 2 + 1 | 6 | 485 KB (threads) | 3.1 s |
| C4 | dashpay/platform#5288 (most comments) | 60 / 170 / 90 | 1 + 1 + 2 + 1 | 5 | 243 KB | 2.6 s |
| C5 | platform#3133, Q-L1 + first pages of Q-L2–Q-L4 in one request | — | 1 request | 1 | 783 KB | 2.8 s |

- Every page costs **1 point**, whatever its nesting (100 threads × 50 comments stays 1). Max comments in one thread:
  7. A full read = 1 point per page; a combined first request (C5) makes a typical PR (< 100 of each) one point.
- Bytes, not points, are the real cost: review bodies of thepastaclaw are 5–10 KB each; a reviews page reached
  926 KB. Server read limits must allow ~1 MB per page (the dashboard client's limits apply).
- Search for the sample (`commenter:thepastaclaw sort:comments-desc`, 2 repositories × 30): 1 point each.

## 2. Watch probe (plan §2.8)

| ID | Batch | Points | Answer | Time |
|---|---|---|---|---|
| P1 | 10 PRs by alias: `headRefOid updatedAt reviewThreads{totalCount} comments{totalCount} reviews(last:1){nodes{id}}` | 1 | 2.4 KB | 1.0 s |
| P2 | 20 PRs, one of them not existing | 1 | 4.7 KB | 1.3 s |

- A missing PR answers `data.<alias>.pullRequest: null` plus a `NOT_FOUND` error with `path [<alias>, "pullRequest"]`;
  the other aliases carry data. The probe must read per-alias errors, not fail the batch.
- `totalCount` without `first` is accepted on these connections.
- 10 PRs per request (plan) costs 1 point; 20 also costs 1. The plan's constant stays (smaller blast radius per
  failed request); 12 points per hour for 10 watched PRs at a 5-minute interval is confirmed.

## 3. `updatedAt` as the change signal

An in-place comment edit bumps the PR's `updatedAt` (S13-1 E5). Probe values `updatedAt` + counts + last review id
therefore catch new threads, new comments, new reviews and the bot's in-place edits.

## 4. Consequences

| ID | Consequence |
|---|---|
| K1 | Plan §4.1 assumption "about one point per page" confirmed **[S-2]**; worst case seen 7 points per full read. |
| K2 | Combine Q-L1 with the first pages of Q-L2–Q-L4 in one request (1 point for most PRs); follow-up pages per connection. |
| K3 | Response size up to ~1 MB per page: the reader needs a size cap of a few MB per page with a clear error, not the 4 KiB notification cap. |
| K4 | Probe: per-alias `NOT_FOUND` handling; batch of 10 kept. |
