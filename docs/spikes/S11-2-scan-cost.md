# S11-2: real cost of the GitHub scan (scan.py + stats.py)

Date 2026-11-18, host `gh` auth as is (read-only; scripts print to stdout, nothing written outside `/data/tmp/s11-2/`).
Instrumented with a counting `gh` shim on PATH (raw logs: `/data/tmp/s11-2/{scan1,scan2,stats1}.log`). Users: the two default logins; issue repos: dash-evo-tool, tenderdash; enumerate-owners unused (see K4).

## 1. Results

| ID | Measure | Run 1 | Run 2 (~5 min later) |
|---|---|---|---|
| R1 | scan.py wall clock | 109 s | 109 s |
| R2 | stats.py wall clock (1 day, 2-day window) | 56 s | - |
| R3 | scan + stats if run back to back | ~165 s | - |
| R4 | Output JSON | 740 KB (100 PRs, 54 issues) | 740 KB, identical summary |
| R5 | Repositories touched | 9 with PRs, 3 with issues, 9 distinct | same |
| R6 | Errors / degraded / stderr | none | none |

Phases of one scan (run 1, seconds from start): PR searches 0-26 (6 searches, 3-5 s spacing); PR detail + `pr list` + REST 26-96 (6 workers); issue searches 70-93 (6, overlapping details); releases/milestones/REST until 109.

## 2. Calls by type (scan.py; identical in both runs)

| ID | Type | Count | Time (sum, parallel) | Note |
|---|---|---|---|---|
| C1 | REST search (`gh search`, 6 PR + 6 issue) | 12 | 18.7 s (1-3 s each; one 7.7 s) | serial, 3 s spacing, peak 6 per 60 s |
| C2 | GraphQL `api graphql` | 102 | 147 s (1.4 s avg, max 3.7 s) | 100 are ONE query per PR (PR_DETAIL_QUERY, 1 PR each, not batched); 2 are milestone batches |
| C3 | `gh pr list --base/--head` (GraphQL inside gh) | 131 | ~41 s window | 1-2 per PR (unblocks / blocked); each is a hidden GraphQL call |
| C4 | other REST (files, releases, milestones, policy) | 48 | 46 s (max 7 s) | 38 are `--slurp` paginated |
| C5 | Total subprocess calls | 293 | | |

stats.py: 12 searches (51 s, 4-5 s apart, peak 12 per 60 s) + 4 GraphQL (5 s). 0 REST.

## 3. Rate limits

| ID | Observation |
|---|---|
| L1 | Search: 0 secondary-limit hits, 0 "cannot be searched", 0 retries in 3 runs. scan peak 6 searches/min; stats 12 in 51 s (3.5 s spacing never tripped). `rate_limit` search counter went 30 -> 24 after a scan. |
| L2 | Core REST: ~57-87 points per scan (counter deltas, noisy: other sessions share the token). |
| L3 | GraphQL: counter delta only ~62 for a scan although 233 GraphQL calls ran (gh counter lag or cheap queries; not explained). Do not rely on it. Account-level GraphQL budget was never a constraint (4.6k of 5k remaining). |
| L4 | Direct cost probe of PR_DETAIL_QUERY via `rateLimit{cost}`, N PRs aliased in one request: N=1 cost 1 (1.7 s, 3 KB); N=5 cost 6 (2.8 s, 188 KB); N=10 cost 12 (5.4 s and 7.8 s, 593 KB); N=20 cost 23 (6.1 s, 717 KB); N=30 returned an HTML error page; N=50 HTTP 504. ~1.1-1.2 points per PR regardless of batch size. |

## 4. Qualifiers (flags of `gh search`; each maps to a search qualifier)

| ID | Qualifier | Result |
|---|---|---|
| Q-a | prs `author:` / `assignee:` / `review-requested:` + `archived:false`, open | author 66 (second login) and 86 total candidates after dedupe; review-requested 18; assignee 1. Second login `review-requested` returned 0 results |
| Q-b | issues `repo:` / `assignee:` / `mentions:` | repo 45+8; assignee 4 (owner), 0 (bot login); mentions 6 (owner), 0 (bot login) |
| Q-c | prs `reviewed-by:` + `updated:>=` (stats) | works; 3 reviewed PRs on the day |
| Q-d | `created:`/`closed:` ranges (stats) | work; `mentions:` is used for issues only, never for PRs |
| Q-e | Note: `gh` adds `is:pr`/`is:issue` itself; scan.py never queries `is:` explicitly | the REST equivalent must set them |

## 5. Issue history cost (drop in v1.1)

| ID | Part | Cost |
|---|---|---|
| I1 | stats.py issue searches (assignee created/closed x2 logins + repos created/closed) | 6 of 12 searches, ~25 s of 56 s (~45%); the other 6 searches are PRs |
| I2 | stats.py GraphQL for issues | within the 5 s |
| I3 | scan.py issue discovery (6 searches, repo list, milestone batch, releases) | ~23 s search time, mostly overlapped with PR details; ~10% of the 109 s |

## 6. Recommendations

| ID | Recommendation |
|---|---|
| X1 | Search pacing: keep 3 s (run, 12-search stats never tripped the secondary limit at 3.5-4.6 s; S-2 may use 2 s in v1.1 but only with the retry ladder 60/120 s kept; 3 s gives ~20/min, below 30). Keep searches serial per user. |
| X2 | GraphQL batch: 10 PRs per request (cost ~12, ~5-8 s, ~0.6 MB). Do not exceed 20; 30+ fails with 502/504. Plan §A value "<=10" is right. Compared with 100 single calls (147 s of worker time) batching cuts requests 10x and wall time to ~15-20 s with 4 concurrent. |
| X3 | Fold `pr list` unblock/blocked (C3, 131 calls) into the batch as planned (Q5/Q6); this is the biggest hidden cost in the skill and is why wall time is 109 s. |
| X4 | Reviews-given history: the Python skill's full stats (without issues) is ~30 s search-only, pacing-bound; the scan alone is ~109 s. With a batched Rust port the scan should land at ~40-60 s, so reviews-given (3-4 searches + 1 GraphQL per login, over a longer chart window ~ +1 search per page) can stay ON by default. Keep the plan rule: if a measured scan exceeds 120 s, turn it off. Drop issue statistics (saves ~25 s). |
| X5 | Whole refresh <= 5 min is comfortable (4x margin). |

## 7. Contradictions with plan section A

| ID | Plan statement | Measurement |
|---|---|---|
| K1 | A.2 Q2 "same selection batched by alias" implies the skill already batches | The skill does NOT: 1 GraphQL per PR, 100 calls. Batching is new work; its ~1.1 pt/PR cost and 20-PR ceiling are measured above. |
| K2 | A.3 "~14-16 searches, 5-10 GraphQL batches, ~20 REST, ~1-1.5 min" | Skill: 12 searches (scan) + 12 (stats) = 24, 233 GraphQL calls, 48 REST, 109 s + 56 s. After porting with batches of 10: 12-14 searches, ~10 PR batches (+ ~10 for unblock/blocked), ~48 REST; estimate fits, REST count is 2x the plan (38 paginated slurps; mostly per-PR files/milestones calls). |
| K3 | A.2 Q12 `reviewed-by:` and `mentions:` need checking | Both work; `reviewed-by` PR search needs `updated:`; mentions is issues only. |
| K4 | Q11/Q14 (unsearchable login fallbacks) | Not exercised: the bot login was searchable today (no "cannot be searched"). |

## 8. Not measured

| ID | What | Why |
|---|---|---|
| M1 | Real GraphQL points per scan | `rate_limit` deltas polluted by other sessions and lagging; only the per-query probe (L4) is reliable. |
| M2 | Behaviour with the App user token (`ghu_`) vs the machine `gh` token | Not to look for or use user tokens; search/visibility may differ (A.1 reach). |
| M3 | Secondary-limit reaction (retry ladder) | Never triggered; code path not exercised. |
| M4 | Q11/Q14 enumeration and events paths | Not triggered (K4). |
| M5 | Stats second run | One run sufficed (no failures); budget. |
| M6 | 2 s pacing | Not tried to avoid tripping the limit on a shared token. |
