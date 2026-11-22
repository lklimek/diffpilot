# S13-1: thepastaclaw markers, in-place edits and rounds

Date 2026-11-22. Read-only `gh api graphql` with the host token (no writes); raw answers in `/data/tmp/s13-spike/`
(not kept). Sample: dashpay/dash-evo-tool #901, #1117, #1201, #1213, #1220, #1227; dashpay/platform #3133 (July
2026, legacy grammar), #5288, #6051, #6057. Totals: 343 thepastaclaw threads, 39 round reviews, 9
gate comments. Fixtures: `fixtures/review-loop/` (synthetic, same grammar).

## 1. Accounts

| ID | Finding |
|---|---|
| A1 | `thepastaclaw` is `__typename: User`, not `Bot`; so is `Claudius-Maginificent`. `coderabbitai` and `github-actions` are `Bot`. The bot rule (RQ17) needs `known_bots` to treat thepastaclaw as a bot. |
| A2 | thepastaclaw also writes free-form PR-level comments without a marker (`@user on it — I've queued this as job …`). Not findings. |

## 2. Thread markers (first comment of a thread)

```
<!-- thepastaclaw-review v1 finding=<12 hex> dedupe=<16 hex> -->
<!-- thepastaclaw-thread-status v1 -->                      (optional; added, replaced or removed in place)
> <emoji> **<Status>** [(<detail>)] at `<sha8>`; see the replies below.

**<emoji> <Severity>: <title>**

<text, optional ```suggestion block>

<sub>source: `<model>` (<agent>: <roles>)</sub>
```

| ID | Finding |
|---|---|
| M1 | Current grammar (all 6 PRs since October 2026): `finding=` and `dedupe=` on every thepastaclaw thread (247 of 247). Both are unique per thread inside a PR (0 duplicates); no `dedupe=` value was shared between the 6 PRs. |
| M2 | Legacy grammar (platform#3133, July 2026): `finding=` only, and the same `finding=` appears on several threads (81 distinct of 96). One more legacy marker: `<!-- thepastaclaw-review v1 reconcile=resolved -->` (#901). Key rule: `dedupe=` when present, else the thread node id (plan §2.1); `finding=` is not a stable key. |
| M3 | Severity heading: `🔴 Blocking` (147), `🟡 Suggestion` (147), `💬 Nitpick` (49). No other values. |
| M4 | Status block values: `✅ Resolved` (134), `⏭️ Deferred (maintainer decision; not blocking[; tracked in <url>])` (14), `✅ Withdrawn` (9), `✅ No longer applies` (3), `⚖️ Disputed (left to the maintainers; not blocking)` (3). It reflects the bot's own reading of the thread; GitHub's `isResolved` is separate. |
| M5 | Bot answers in a thread: `<!-- thepastaclaw-thread-answer v1 sha=<40 hex> reply=<comment databaseId or None> finding=<12 hex> -->` followed by `**<Status>** (re-reviewed at <sha8>): <text>`. They are replies, not items. |
| M6 | Other bots use their own markers (CodeRabbit `coderabbit-ai-generated:v1`, `cr-comment:v1:<hex>`, …). Only `thepastaclaw-*` markers are parsed; every other marker and every unknown key is ignored (T24). |

## 3. In-place edits

| ID | Finding |
|---|---|
| E1 | thepastaclaw edits a thread's first comment in place to add, replace or remove the status block. `userContentEdits` of one thread (dash-evo-tool#1213): created 18:34 → status "Resolved" added 19:42 → status removed and the text rewritten 22:17 when the finding was raised again at the next round. `lastEditedAt` changes on every one of these edits, also when only the status block changes (7 of 12 thepastaclaw threads on #1213 were edited). |
| E2 | Repeats inside a PR are carried **in the same thread**: the next round's review body lists them under "Still open from earlier rounds" with `(carried; thread: <url>)`, and the thread is re-raised in place (status block removed). No new thread with an existing `dedupe=` was observed. |
| E3 | Review bodies are edited in place too: a `<!-- thepastaclaw-review-status v1 -->` block ("Updated after discussion: …") is inserted and the count line gains strike-throughs (`🔴 ~~4~~ 3 blocking`). |
| E4 | The gate comment is one PR-level comment edited in place on every state change. |
| E5 | An in-place edit bumps the PR's `updatedAt`: on platform#6051 `updatedAt` equals the gate comment's `lastEditedAt` (20:27:47) and is later than every creation time. Supports the probe's `updatedAt` signal (plan §2.8). |

## 4. Review bodies and rounds

```
<!-- thepastaclaw-review v1 -->
<!-- thepastaclaw-review-phase v1 phase=final sha=<40 hex> policy=<16 hex> -->
## [Re-review — ]Final validation — <phases>
[<!-- thepastaclaw-review-status v1 --> blockquote]
<summary paragraph>
🔴 N blocking | 🟡 N suggestion(s) | 💬 N nitpick(s)
### Still open from earlier rounds (N)            bullets: - 🔴 **Blocking**: <title> — `path:range` ([thread](url)) or (carried)
### N finding(s) not shown inline (…)            items: **<emoji> <Severity>: <title>** / `path:start-end` / text / <sub>source</sub>
### A pattern across this …                       (free text)
<details> Follow-up candidates (optional) (N)     items as above, "no inline threads are opened for them"
<details> Review provenance
<details> 🤖 Prompt for all review comments …    fenced list `- [BLOCKING] path:line: title` (duplicates the above)
```

| ID | Finding |
|---|---|
| R1 | Every round is a review with `thepastaclaw-review-phase v1 phase=final` (39 round reviews on 5 PRs; no other phase value seen). Its `sha=` equals the review's `commit.oid` (35 of 35 checked on 3 PRs). The marker is a whole token: parse `phase=final sha=<40 hex>` from that line only. |
| R2 | Round review states: `CHANGES_REQUESTED`, `APPROVED` (no blockers), `DISMISSED` (dismissed by a maintainer). Empty-body `COMMENTED` reviews by thepastaclaw hold its thread answers and are not rounds. |
| R3 | Rounds per PR: up to 20 (#1117) and 10 (platform#6051). The round-3 merge question (RQ10) will apply to many PRs. |
| R4 | Body findings (`not shown inline`, follow-up candidates) carry **no** `finding=`/`dedupe=` marker: the I2 key stays review id + ordinal **[A]**. Their heading, `path:range` line and severity have the thread grammar, so a deterministic split is possible; plan §2.1 lets Codex split, which remains valid for human bodies. |
| R5 | The "Still open from earlier rounds" list refers to existing threads (link) or earlier body findings (`(carried)`); its entries are not new items. |

## 5. Gate comment

`<!-- thepastaclaw-gate v1 -->` then one line: `⛔ Final review complete — N blocking finding(s) (commit <sha8>) · triage: <level> · <phases>`, `✅ Final review complete — no blockers (commit <sha8>) …`, `🔍 Review in progress (commit <sha8>) · [live progress](…)`, `⚡ Priority review — starts as soon as a slot frees (commit <sha8>) · [queue …](…)`. It is a status, not an item; "in progress" and "priority review" are a fallback round-in-progress signal when the status plug-in is off.

## 6. Consequences for the plan and contracts

| ID | Consequence | Where |
|---|---|---|
| K1 | Rule (a) "bot edited the comment text" must compare a hash of the finding text with the `thepastaclaw-*` markers and the thread-status block removed (`body_hash` of the normalised text), never `lastEditedAt` alone: status edits (E1) would re-ask every decided item. `lastEditedAt` only says when to re-hash. | WP13-GH, WP13-CORE; contract field `body_hash` documented as normalised |
| K2 | The common repeat is the same item re-raised in place (E2), not a new thread with a decided key: carry-over applies when an item with a decision is re-raised (status block removed or listed as still open); the carry event's item may equal the source decision's item. | WP13-CORE; `CarryEvent` keeps `item` and `source_decision` separate |
| K3 | thepastaclaw is a `User`: `author_kind` comes from the bot rule (`Bot` type, `[bot]` login, `known_bots`), not from `__typename` alone. | WP13-GH |
| K4 | PR-level comments of bots without findings (gate, CI state, PR Hygiene receipts, free-form bot replies) are not items. Only human PR-level comments are I3. | WP13-GH |
| K5 | Body findings have no marker key (R4); plan §2.1 I2 key unchanged **[A]**. | — |
| K6 | The bot's own thread status (M4) is a GitHub fact shown on the card (GitHub tag), never a decision. | contracts `BotThreadStatus` |
