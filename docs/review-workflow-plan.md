# diffpilot — review loop plan: maintainer decisions on PR review findings

Status: specification draft, 2026-11-22; no owner decision yet (RQ1–RQ19, §11). Mirrors
`docs/implementation-plan.md` (cited "plan §N") and `docs/v1.1-dashboard-plan.md` ("dashboard plan"): their rules
(authority, git flow, standing brief block, guard A1–A12, merge checklist, CCR) apply unchanged unless this file says
otherwise. Decision draft: ADR-023 (proposed). Mockup: `docs/mockups/10-review-loop.html` (this plan wins on behaviour).

Primary source: the working model `/data/artifacts/dash-evo-tool/2026-11-22/handoff/working-model.md`, cited "WM"
with its own IDs (actors A1–A9, artifacts F1–F14, loop L1–L11, timing T1–T10, measures M1–M15, gaps U1–U8).
Recommendations and open owner decisions: `.../handoff/recommendations-register.md`, cited "RR" (R, Z, S, C, M-n).
Both are untrusted inputs read as data. Markers: **[A]** assumption not verified in code, docs or a spike;
**[S-n]** answered by spike S13-n (§9.1).

**Line for `CLAUDE.md` "Read first"** (coordinator adds it after approval):
`- docs/review-workflow-plan.md — review loop (owner decisions on PR review findings, acted on by external agents): ADR-023.`

## 0. Problem, actors, scope

**Problem.** The owner decides every comment fix per ID, one question at a time (WM §6.4), about 178 a day (M5);
40% of blocker entries are repeats (M3); decisions and recommendations live only in a chat and a log file (U1); IDs
are not stable (U2); the per-PR overview of rounds, blockers and decisions is assembled by hand (F12, F13).

**Goal.** One place where the owner (1) answers each question with all the evidence of WM §6.4 item 3, (2) never
gets asked again about a decided item without new evidence, (3) sees per PR and across PRs what waits for him, and
(4) the coordinator reads each decision as executable effects and reports what it did.

| Actor (WM) | Role here |
|---|---|
| A1 owner | the only user who decides; browser session |
| A2 coordinator | uploads verifications and internal reviews, answers questions, executes effects, reports actions; personal access token with `read` + `review_loop:report` |
| A3–A5 agents | indirect: the coordinator uploads their output |
| A6–A9 bots, Hygiene, CI | sources read by the server from GitHub and the PR Hygiene plug-in (ADR-022) |

**In scope:** tracked PRs, findings from the four sources of WM §6.1, verification, decisions, effects, actions,
repeats, rounds, history, the "waiting for you" list. **Not in scope:** any GitHub write by diffpilot in phases 1–2;
fixing code; running reviews; deciding for the owner (no auto-decline, RR C-3); bot configuration (RR R3, R4, R13);
WM §10 actions.

## 1. User stories (Given / When / Then)

| ID | Actor | Story |
|---|---|---|
| US1 | A1 | **Given** PR 482 has 5 undecided findings with verifications, **when** the owner opens its review loop, **then** one question is shown with PR number and purpose, item ID, source and author kind, category, problem and impact, pre-existing, necessity verdict with reason, tripped gates, fix size, local-fix state (WM §6.4 item 3), the recommended option first and marked "Recommended", and "1 of 5". |
| US2 | A1 | **Given** a question, **when** the owner picks a decision and confirms, **then** the decision is stored with the recommendation shown at that moment, the next question opens, and the coordinator's feed lists the decision with its effects within one SSE event. |
| US3 | A1 | **Given** three items share the proposed disposition "reject: outside the threat model", **when** they come up, **then** one group question lists them with one decision for all and a "Discuss individually" action per row that turns that row into its own question; each item still gets its own decision record (WM §6.4 items 5, 7). |
| US4 | A1 | **Given** an item whose verification names gate G-FUNDS, **when** the owner rejects it, **then** the effects contain a reply and no resolve unless "Resolve this thread" was ticked (WM §6.5 special case 1); gate items are never grouped (RQ4). |
| US5 | A1 | **Given** a new bot thread carries the `dedupe=` marker of decided item C7, **when** the server reads the PR, **then** it is listed as "repeat of C7" with C7's decision, is not asked, and the coordinator gets the carried effects; if the bot edited the text after the decision or the verification reports new evidence, it is asked again (WM §6.3; RQ3). |
| US6 | A1 | **Given** a question, **when** the owner writes "why is this pre-existing?" and sends it as a question, **then** the item moves to "Waiting for the coordinator", the coordinator gets the question in its feed, and its answer returns the item to the queue with the same options (WM §6.4 item 6). |
| US7 | A2 | **Given** a token with `review_loop:report`, **when** the coordinator uploads verification items for head `9f3c2e1`, **then** the server validates them, matches them to items by key, answers with new, updated and rejected rows (rejections name the field), and refuses items of another PR. |
| US8 | A2 | **Given** decisions with pending effects, **when** the coordinator reads the feed, **then** each effect has its kind, parameters (reply text, thread ID, first-comment `databaseId`, target PR) and state; **when** it reports "resolve done", **then** the effect is done and the next read of GitHub shows the thread resolved as a fact. |
| US9 | A2 | **Given** a `review_loop:report` token, **when** it calls the decision route, **then** the answer is 403 `token_not_accepted` and nothing is stored. |
| US10 | A1 | **Given** the owner is deciding C9, **when** a new verification for C9 arrives, **then** "Decide" answers 409 `item_changed`, the card reloads with the new evidence marked, and nothing is stored. |
| US11 | A1 | **Given** a decided item whose effects were partly done, **when** the owner changes the decision, **then** a superseding decision is stored, undone effects are cancelled, done ones stay listed, and the card says which effects already reached GitHub. |
| US12 | A1 | **Given** several tracked PRs, **when** the owner opens "Waiting for you", **then** one list shows per PR the undecided questions, the coordinator's questions back, and WM §10 actions the server can derive (Hygiene shows only the human, a bot `CHANGES_REQUESTED` whose items are all decided), each with its source; never an all-clear. |
| US13 | A1 | **Given** a PR, **when** the owner opens its round history, **then** each bot round (head, bot verdict, time) lists new, still-open and repeated items with their decisions; each item shows every round it appeared in (WM §15 view 5). |
| US14 | A1 | **Given** threads exist but no verification was uploaded, **when** the PR is opened, **then** those threads are listed as "Not assessed by an agent" questions without a recommendation, never hidden. |
| US15 | A1 | **Given** GitHub cannot be read (rate limit, expired sign-in), **when** the page loads, **then** the last read is shown with its time and the failure in plain words ("GitHub rate limit: next read at 10:52"). |

## 2. Workflow semantics

### 2.1 Items and sources
| ID | Source (WM §6.1) | Read by | Stable key (ADR-023 §6) | Enters the queue |
|---|---|---|---|---|
| I1 | review thread (F3) | server, GitHub | `dedupe=` marker, else thread node id | yes |
| I2 | review-body item (F4: `CHANGES_REQUESTED` bodies, latest human review, bot body findings without thread) | split by the verifier, uploaded | source URL + verifier ordinal **[A]** | yes |
| I3 | human PR-level comment | split by the verifier, uploaded | comment URL + ordinal | yes |
| I4 | internal review finding (F7) | uploaded report | report id + finding id; repeats via the reviewer's prior-decision brief (Z1′) | only WM §6.6 item 2 exceptions; others read-only |

Bot or human: GitHub `Bot` type, a `[bot]` login, or the user's `known_bots` setting (dashboard K11; WM §1 note:
`claude` is not a bot). Items already resolved on GitHub without a decision here are counted, not asked (WM §6.3).

### 2.2 Verification (uploaded per item, WM §6.2, claudius T-3c2b5f03)
Fields: thread node id, first-comment `databaseId`, validity (`valid`, `partly_valid`, `invalid`) with `file:line`
evidence, pre-existing on base (yes/no), category (closed catalog of claudius #118, 17 slugs), necessity verdict
(`in` with evidence / `out` with reason and a better-fitting PR), tripped gates (`severity` §2 IDs), elsewhere
(PR, issue, upstream; local branches only as "local", never published), fix sketch and size (`XS`/`S`/`M` + lines),
local fix commit if one is held (WM §6.5 special case 6), reply drafts (one or two variants), effect of earlier
decisions, recommendation (decision kind, reason, target), reporter and head SHA. All of it is agent (LLM) text except
IDs and SHAs. A verification at an older head than the PR's is shown as "assessed at abc123; PR now at def456".

### 2.3 Decisions and effects (WM §6.4 item 4, §6.5)
| ID | Decision | Required input | Effects (bot thread) | Human thread differs |
|---|---|---|---|---|
| D1 | Resolve (already fixed) | — | resolve | reply what was done, then resolve |
| D2 | Fix here | optional note | fix (test red first) → after the push: reply with commit → resolve | no resolve unless ticked |
| D3 | Reject | reason (prefilled from the recommendation) | reply reason → resolve; optional "Known limitation" body edit | no resolve unless ticked |
| D4 | Handled in #N (move) | open PR number | reply "Per maintainer decision, this is addressed in #N…" → resolve | same |
| D5 | Follow-up PR | new or existing #M | open draft skeleton PR first → reply link → resolve | same |
| D6 | Upstream issue | issue or PR URL (or "create", WM §10 item 9) | reply link → resolve; "Known limitation" body edit | same |
| D7 | TODO | title (prefilled) | save `memcan:todo` → reply "Per maintainer decision, out of scope for this PR." (never the TODO) → resolve; save failed → no reply, no resolve | same |

- **Override flag** (WM §15 Decision): `necessity_gate` (fix although the verdict is `out`) or `blocker_gate` (the
  §6.4 option "accept despite the gate", modelled as D3 with the gate named; meaning to confirm, RQ6).
- **Gate threads:** with a gate from the verification or marked by the owner, no resolve effect exists unless the
  decision ticks "Resolve this thread"; D1 counts as ticked.
- **Body and PR-comment items** have no thread: their replies are batched into one PR-level comment per source review
  (effect `pr_comment`). **Premise false** is D3 with evidence as the reason. **Held local fix:** D3 adds "reset to the
  pushed head", D4/D5 "move the commit to a new branch".
- Effects are stored at decision time; the coordinator reports each as `done` (with a reference: commit SHA, reply
  URL, PR number, upstream URL, `todo saved` without id on GitHub) or `failed` (plain reason). Replies are in English
  and say "per maintainer decision" (WM §6.5).

### 2.4 Queue order and groups
One question at a time; the order is deterministic and shown in the queue list:
1. Items with new evidence after a decision (bot edited, new comment, verification says so); 2. coordinator answers to
the owner's questions; 3. gate items; 4. items the reviewer marked Blocking (bot's claim, WM §11), by first round,
then ID; 5. other single items; 6. groups. A **group** = two or more non-gate items with the same recommended
decision kind and the same reason class or target PR; it offers one decision for all and "Discuss individually" per
row (WM §6.4 item 5). Unassessed items are never grouped. Owner shortcuts: "Skip for now" (to the end), "Decide the
rest per recommendation" (WM §6.4 item 9; per-ID records with `via = bulk`; gate items excluded; RQ5).

### 2.5 States
Item: `undecided` → `asked_back` (owner question pending) → `decided` → `acting` (some effects pending) → `done`
(all effects reported) ; `repeat` (carried); `reopened` (new evidence); `withdrawn` (thread deleted, or resolved on
GitHub without a decision here: listed, never silently dropped). Decision: append-only; a change supersedes.

## 3. Views and flows (WM §15 views 1–5)

Global rules: brief §4 rules 1–3 (full width, tooltips on every label), loading / empty / error states, keyboard,
WCAG 2.2 AA (§3.9). Navigation: app shell item "Review loop (N)" (N = undecided questions, no other badge).

| ID | View | WM §15 | Route (web) | Phase |
|---|---|---|---|---|
| V1 | Question (one item) / group question | view 1 | `/review-loop/:pr` (default tab "Questions") | 1 |
| V2 | PR table: decided, repeated, resolved, internal separated | view 2 | `/review-loop/:pr?tab=table` | 1 |
| V3 | Waiting for you, across PRs | view 3 | `/review-loop` | 1 (decisions, Hygiene-derived); 2 (push state, `ghsudo`) |
| V4 | Push and gate state per PR | view 4 | PR header + V3 column | 2 |
| V5 | Rounds and per-item history | view 5 | `/review-loop/:pr?tab=rounds`, item history panel | 2 |
| V6 | Decision log per PR | (U1) | `/review-loop/:pr?tab=log` | 1 |

`:pr` = dashboard item key `<owner>~<repo>~pr~<n>`.

### 3.1 V1 Question
Layout: left, the queue list (sections "Needs you", "Waiting for the coordinator", "Repeats (not asked)",
"Decided"); centre, one card; right, the PR panel (V4 header, Hygiene, counts). Card, top to bottom (evidence first,
brief §1):
1. Header: `C14 · 3 of 7`, source and author ("thepastaclaw · bot · inline thread"), first seen (round, head), the
   bot's own severity marker as a GitHub-tagged claim ("thepastaclaw: Blocking"), gate chip ("Gate: G-FUNDS ·
   from the verification" / "· marked by you"), category chip, repeat-of link.
2. The comment (GitHub tag; plain text, links only to `https://github.com/…`), "Open on GitHub".
3. Location (file:line link at the PR head) and, when an analysis of this head exists, "Open in Files" (phase 3).
4. Facts read by diffpilot (badges name the check): "Thread unresolved · read 10:41", "Outdated", "Edited by the bot
   after your decision", "Marker `dedupe=…`".
5. Verification (LLM tag, "from <reporter> at 9f3c2e1, 10:32"): validity and evidence, pre-existing, necessity
   verdict with reason, tripped gates, elsewhere, fix sketch and size, local fix held, effect of earlier decisions.
6. Options: a radio group, the recommended one first with "Recommended" (written, not colour only); D1–D7; inputs
   appear for the chosen one; override select; for gate items an unticked "Resolve this thread" with "Gate thread
   (G-FUNDS): resolved only if you tick this".
7. "What happens next": the computed effects in order, the reply text (editable, public, "posted by the
   coordinator's GitHub account").
8. Actions: **Decide** (Enter), **Ask the coordinator** (opens a text field; sends a question, US6), **Skip for now**.
After Decide: the next card opens, focus on its heading, a live region says "C14: Reject recorded. 6 questions left.";
"Undo" stays in a toast until any effect of that decision is reported done (then only a superseding decision).

**Group card:** title "Same recommendation: Reject — outside this PR's threat model (3 items)", a table (ID,
reviewer, title, location, verification one-liner, "Discuss individually"), one reason field, the effects per row,
Decide. A row taken out goes next in the queue.

**Empty:** "No undecided questions in what was read at 10:41: 14 threads (all read), 1 verification upload at
9f3c2e1. 2 threads have no verification and are listed." **Error:** "GitHub could not be read (rate limit). Showing
the read of 10:12; next read at 10:52."

### 3.2 V2 PR table
Columns of WM §6.3 (ID, source, category, problem, scope, elsewhere, verification one-liner, recommendation) plus
decision and effect state; sections Undecided, Decided, Repeats, Resolved on GitHub (count, expandable), Internal
review (read-only: finding, coordinator's disposition). Search and filters (category, state, author kind); sticky
header; real `<table>`.

### 3.3 V3 Waiting for you
One row per item needing the owner: tracked PR, what ("3 questions", "1 answer to read", "Hygiene: only owner
approval pending → `/self-reviewed` or merge", "thepastaclaw CHANGES_REQUESTED; every item decided → dismissal is
yours", phase 2: "2 pushes held: bot reviewing since 10:12", "`ghsudo` approval pending") and a link (diffpilot page or
GitHub). Derived rows name their rule in a tooltip. diffpilot never performs these WM §10 actions.

### 3.4 V4 PR header
Title, `#N`, base (stack parent when the base is another PR's branch), draft, head SHA, CI (existing component),
round count against the PR's round budget when set (WM T8; no action defined, RQ10), PR Hygiene note (ADR-022,
attributed, with its waiting/stale states), counts (undecided, waiting for the coordinator, effects pending, repeats,
unresolved threads read from GitHub). Phase 2: bot gate state (WM F5, F6: "thepastaclaw reviewing since 10:12 · push
held" from the gate comment; the status feed only if RQ16) and held pushes reported by the coordinator.

### 3.5 V5 Rounds and history
A round = one submitted review whose body carries thepastaclaw's `phase=final sha=<head>` marker (WM F4) **[S-1]**.
Table: round, head, time, bot verdict, new, still open, repeats, then per decision kind and undecided (counts, never
percentages). A row expands to its items. Internal reviews are a collapsed list (range, reviewer, model, time,
findings, links; WM §15 Internal review, Z12 fields optional). Item history panel: every round the key appeared in,
every decision and effect, in time order.

### 3.6 V6 Decision log
Chronological: decision, item, by, at, recommendation at that moment, `via` (single, group, bulk, carried), override,
effects with their state, reporter and reference. This is the structured store WM U1 asks for.

### 3.7 Wording (brief §1; `scripts/wording-rules.json` covers `components/review-loop/**`, `screens/review-loop/**`)
- Agent text: "LLM" tag plus reporter; tooltip "written by an LLM agent, not checked by diffpilot". Validity reads
  "The verifier says: valid / partly valid / invalid". Never "verified", "confirmed", "safe", "no risk", "noise".
- A bot's severity and "blocking" are the bot's words with the "GitHub" tag (WM §11). Report floats,
  `overall_severity` and `ai_verdict_confidence` are never shown; an LLM severity band, if shown, reads "LLM rated
  MEDIUM (not checked)".
- The coordinator's "done" reports read "The coordinator reports: replied (link)"; a matching GitHub read adds the
  fact badge "Reply found on GitHub · 10:44".
- Counts only; no agreement percentage.

### 3.8 Keyboard
`j`/`k` next/previous question, `1`–`7` choose D1–D7, `Enter` decide, `i` discuss individually (group row focused),
`q` ask the coordinator, `s` skip, `g` open on GitHub, `t`/`r`/`l` tabs table/rounds/log, `?` help, `Esc` close. None
while typing.

### 3.9 Accessibility
Card heading receives focus on load; decisions are a `radiogroup` with labelled options and the recommendation in the
accessible name; gate checkbox described by its explanation (`aria-describedby`); live region for outcomes and
counts; tables with headers; targets ≥ 24 px (WCAG 2.5.8); focus never hidden by the sticky header (2.4.11); no
colour-only meaning; axe on every state, both themes.

## 4. Data model and sources

### 4.1 GitHub reads (server, the owner's user token, queries only; T18 rule)
| ID | Read | Purpose |
|---|---|---|
| Q-L1 | `pullRequest{title state isDraft baseRefName headRefName headRefOid reviewDecision}` | V4 |
| Q-L2 | `reviewThreads(first:100){id isResolved isOutdated path line startLine comments(first:50){databaseId author{login __typename} body createdAt lastEditedAt url pullRequestReview{id}}}`, paginated, cap 1000 threads | I1, markers, edits, replies |
| Q-L3 | `reviews(first:100){id author state submittedAt commit{oid} body url}` | rounds (F4), I2 sources, dismissal hint |
| Q-L4 | `comments(first:100){databaseId author body createdAt url}` | gate comment (F5), I3 sources |
Read on page open when the last read is older than 60 s, on "Read GitHub again", after an upload and after an action
report (debounced 10 s per PR). Marker parsing (`dedupe=`, `finding=`, severity marker, `phase=final sha=`) is
deterministic, in `server::review_loop::markers`, tested on recorded bodies **[S-1]**. Cost per read: ≤ 1 point per
page **[A]**.

### 4.2 Tables (one migration `NNNN_review_loop.sql`; every table has `tenant_id`; rows scoped by `user_id` like T16)
| ID | Table | Key columns |
|---|---|---|
| T-L1 | `review_loop_prs` | id, user_id, repo, number, tracked_at, round_budget (null), last_read_at, read_state, `github_snapshot` JSON (Q-L1–Q-L4 digest) |
| T-L2 | `review_loop_items` | id, pr_id, display_no (unique per PR), source_kind, stable_key (unique per PR), author, author_kind, path, lines, title, url, first_seen_head, first_round, bot_severity, gate_marks JSON (source each), state, revision, repeat_of |
| T-L3 | `review_loop_uploads` | id, pr_id, kind (`verification`, `internal_review`), head, token_id, received_at, body JSON (≤ 2 MiB), range, reviewer, model, started_at, finished_at, links |
| T-L4 | `review_loop_verifications` | id, item_id, upload_id, fields of §2.2, recommendation JSON |
| T-L5 | `review_loop_questions` | id, item_id, asked_by, text, asked_at, answer, answered_by_token, answered_at |
| T-L6 | `review_loop_decisions` | id, item_id, user_id, kind, reason, target, override, resolve_ticked, reply_text, recommendation JSON, via, carried_from, supersedes, decided_at (append-only) |
| T-L7 | `review_loop_effects` | id, decision_id, seq, kind, params JSON, state, reference, message, token_id, idempotency_key, updated_at |
| T-L8 (phase 2) | `review_loop_appearances` | item_id, round head, review id, status (`new`, `still_open`, `repeat`) |
Retention: everything while the PR is tracked; untracking asks for confirmation; after merge or close RQ14. Internal
findings of I4 live in their upload document; only escalated ones become T-L2 rows.

### 4.3 Coverage of WM §15
| WM entity / view | Here |
|---|---|
| PR (head, base, draft, stack parent, round count, Hygiene) | T-L1 + snapshot; Hygiene from the plug-in (in memory, ADR-022) |
| Round (head, bot verdict, started, finished) | from Q-L3 markers; "started" from the gate comment (F5) in phase 2; finished = review `submittedAt` |
| Finding (source … elsewhere link) | T-L2 + T-L4 |
| Decision (recommendation, disposition, reason, time, override) | T-L6 |
| Action (reply text and id, resolve, commit, follow-up PR, body edit, TODO id; done or pending) | T-L7 (TODO id stays in diffpilot, never on GitHub) |
| Internal review (range, reviewer, model, links, findings) | T-L3 `internal_review`; links rendered only for an allowed origin (RQ19) |
| Gate state (bot reviewing, since, push held) | phase 2: gate comment read + coordinator status report (RQ15) |
| Views 1–5 | V1, V2, V3, V4, V5 |
Not modelled: CI ledger (F14) and the tracking log (F9) itself — the log is what V6 replaces; the ledger stays the
coordinator's evidence.

## 5. How decisions reach the agents that act

### 5.1 Who acts
| Effect | Phase 1–2 | Later |
|---|---|---|
| fix, commit, push (WM L9, L10, §8) | coordinator and its implementer, own worktrees, `ghsudo` | unchanged; diffpilot never pushes |
| reply, resolve, PR-level comment, body edit | coordinator, its GitHub account | phase 3 option: server, server-held credential (RQ2) |
| follow-up PR skeleton, upstream issue link, TODO | coordinator | unchanged |
| verification | coordinator's verifier (A5) | phase 3 option: diffpilot `verify_comments` job, read-only runner, PR analytic session (CCR) |
| WM §10 actions | owner only, on GitHub | never in diffpilot |

### 5.2 Agent protocol (personal access token, ADR-019 + scope `review_loop:report`)
1. Upload verifications per PR after L6 (RL6). 2. Hold one SSE stream (RL11) or poll the feed (RL9) with `revision`
and `ETag` (dashboard E5-5). 3. For each decision execute the effects in `seq` order and report each (RL10, idempotency
key). 4. Answer owner questions (RL8). 5. Upload each internal review report right after the round (RL6, kind
`internal_review`; archiving to `/data/artifacts` (WM F7) stays). Deep link for the `summon` ask:
`/review-loop/<pr>`.

### 5.3 What must change for agents that write replies or resolve threads
Nothing in the runner. The diffpilot agent keeps its read-only, PR-scoped installation token (T3, ADR-005). External
agents keep their own credentials outside diffpilot. If the owner wants diffpilot to post (RQ2): the server performs
the GraphQL mutation (`addPullRequestReviewThreadReply`, `resolveReviewThread`) with the owner's App user token (reply
shown as the owner via the App) or an installation token with `pull_requests: write` (shown as the App bot) — spike
S13-3 — only for effects of a stored owner decision, serialised per PR like ADR-006, never from an agent request.

### 5.4 Owner-only
Every decision, supersede, reopen, gate mark and gate clear, "Resolve this thread", round budget, tracking and
untracking. Tokens get 403 `token_not_accepted` on all of them (contract test over `ROUTES`).

## 6. Contracts and routes

Types in `crates/contracts/src/api/review_loop.rs`: `LoopPrSummary`, `LoopPrPage`, `LoopPrOverview`, `LoopItem`,
`ItemSource`, `Verification`, `Recommendation`, `DecisionKind` (`resolve`, `fix_here`, `reject`, `handled_in`,
`follow_up_pr`, `upstream_issue`, `todo`), `Override`, `Decision`, `Effect` (`kind`, `seq`, `params`, `state`),
`ActionReport`, `OwnerQuestion`, `Round`, `LoopEvent`, upload types `VerificationUpload` and `InternalReviewUpload`
(the latter embeds the claudius report as opaque JSON parsed tolerantly). Integers only on the wire; open enums
(tolerant readers, E5-3); JSON Schemas for `read` responses and both uploads in `schemas/api/`.

| ID | Method | Path (`/api/v1/review-loop…`) | Auth | Request → Response |
|---|---|---|---|---|
| RL1 | GET | `/prs` | read | → `LoopPrPage` (V3 rows included) |
| RL2 | POST | `/prs` | session_csrf | `{url}` → 201 `LoopPrSummary` (track) |
| RL3 | GET / DELETE | `/prs/{pr}` | read / session_csrf | → `LoopPrOverview` / 204 (untrack) |
| RL4 | POST | `/prs/{pr}/read` | session_csrf or `review_loop:report` | → 202 (read GitHub again; 429 when < 10 s) |
| RL5 | GET | `/prs/{pr}/items` | read | `state`, `kind` → `LoopItem[]` with verifications, decisions, effects |
| RL6 | POST | `/prs/{pr}/uploads` | `review_loop:report` or session_csrf | upload → 201 receipt `{upload_id, new, updated, rejected[{row, field, message}]}`; 422 `invalid_upload`; 413 |
| RL7 | POST | `/items/{id}/decisions` | **session_csrf only** | `{kind, reason?, target?, override?, resolve_ticked, reply_text?, item_revision}` → 201 `Decision`; 409 `item_changed` |
| RL7b | POST | `/prs/{pr}/group-decisions` | **session_csrf only** | items with revisions + one decision → 201 `Decision[]`; 409 lists changed items |
| RL8 | POST | `/items/{id}/questions`, `/questions/{qid}/answer` | session_csrf / `review_loop:report` | text → 201 |
| RL9 | GET | `/decisions` | read | `pr`, `state=pending|any`, `since`, `cursor` → decision feed with effects; `ETag` |
| RL10 | POST | `/effects/{id}/reports` | `review_loop:report` or session_csrf | `{outcome, reference?, message?, idempotency_key}` → 201 |
| RL11 | GET | `/events` | read | SSE `LoopEvent` (`decision`, `question`, `pr_read`, `heartbeat`) per user |
| RL12 | PUT | `/prs/{pr}/settings` | **session_csrf only** | `{round_budget}` → 204 |

`RouteAuth` gains `SessionCsrfOrToken(TokenScope::ReviewLoopReport)`; `TokenScope` gains `review_loop_report`
(Settings token dialog: scope checkboxes). Error codes: `item_changed`, `invalid_upload`, `upload_too_large`,
`pr_not_tracked`. ADR-019 amendment: "tokens never on mutating routes" becomes "only on routes marked with a write
scope".

## 7. Threat model additions
| ID | Control | Test |
|---|---|---|
| T22 | Decisions, gate marks and settings only with session + CSRF of an allow-listed user | `no_token_scope_reaches_decision_routes`, `gate_resolve_needs_explicit_tick` |
| T23 | Uploads: size cap, schema validation, PR and head bound, text-only rendering, links only `https://github.com/…` or an allowed origin (RQ19), control and bidi characters stripped as in ADR-022 §7 | `upload_for_other_pr_rejected`, Playwright `loop_texts_render_as_text` |
| T24 | Bot and human comment bodies are untrusted (G-PHISH): plain text; marker parsing never executes or follows content | `markers_parse_only_known_keys` |
| T25 | Write-scope token: audit line per request, rate limit as ADR-019, revocation; cannot list tokens | ADR-019 cases extended |
| AR-11 | Accepted (proposal): a coordinator token can upload misleading verifications (prompt injection via bot text into the verifier); mitigations: owner decides, evidence shown, LLM tag, facts read by the server | — |

## 8. Reuse, supersede, integrate
| Existing piece | Use |
|---|---|
| claudius `triage-findings` | **superseded** for PR-comment decisions on tracked PRs (ADR-023 §11): no cross-round memory, no queue order, action set lacks D4–D7 and overrides. Kept for local review reports without a PR. |
| claudius `check-pr-comments` | its §8 table-mode matrix becomes §2.3 (extended by WM §6.5); it gains a diffpilot mode (external work, RQ12) |
| dashboard (ADR-016/017) | item keys for `:pr`; `known_bots`; app shell; a work item chip "3 questions" linking to V1 (phase 2) |
| PR Hygiene plug-in (ADR-022) | PR header note and the V3 "only owner approval pending" row; activity marking extended (ADR-023 §13) |
| Access tokens (ADR-019) | agent auth; one new scope |
| `github::threads` | extended query (all comments, edits, review link) in `github::review_loop`; Tour threads unchanged |
| Flows / Files / Ask | phase 3: "Open in Files" at the comment's lines; Ask about the comment's lines; the Inspector's GitHub-thread section shows the decision state |
| Components | `badge`, `LlmLabel`, `banner`, `tooltip`, `popover`, `help`, `flash`, `GithubMark`, CI status, plug-in note |

## 9. Phasing and work packages

**Phase 1 (smallest useful): decide in diffpilot, act from the feed.** RL1–RL12 except V4 gate state; V1, V2, V3
(decisions + Hygiene rows), V6; markers and repeats (I1 keys); internal-review upload with escalated items; SSE.
Useful as soon as the coordinator uploads and reads (claudius side, RQ12). **Phase 2:** V5 rounds and history (T-L8),
V4 gate and push state, coordinator status report, effect observation badges, dashboard chips, browser
notifications (RQ18). **Phase 3 (each an owner decision):** server-side reply/resolve (RQ2, S13-3), diffpilot
verification job (CCR: `JobKind`), Flows/Files/Ask integration.

### 9.1 Spikes
| ID | Question | Gates |
|---|---|---|
| S13-1 | Recorded thepastaclaw threads, reviews and gate comments of 3 PRs: marker grammar (`dedupe=`, `finding=`, severity, `phase=final sha=`), in-place edits, "Still open from earlier rounds" in bodies; fixtures | markers, V5 |
| S13-2 | Owner token cost and caps of Q-L1–Q-L4 on the largest PR (threads > 100, comments per thread > 50) | WP13-GH |
| S13-3 (phase 3) | Can the App user token reply and resolve on a governed repo; what identity does GitHub show | RQ2 |

### 9.2 Work packages (phase 1; prefix WP13, sizes in lines)
| WP | Goal | Owned paths | Depends | Sec | Size |
|---|---|---|---|---|---|
| WP13-A | contracts, routes, error codes, uploads' schemas, migration, `docs/contracts/*` additions, T22–T25 | `contracts/src/api/review_loop.rs`, routes, `crates/db/migrations/NNNN_review_loop.sql` | approval, S13-1 | yes | ~800 |
| WP13-GH | Q-L1–Q-L4, markers, fake-github additions | `server/src/github/review_loop/**`, `tools/fake-github/**` (new files) | A, S13-2 | yes | ~600 |
| WP13-DB | repository API, revision checks, append-only decisions | `db/src/repo/review_loop.rs` | A | — | ~600 |
| WP13-CORE | ingest, key matching, repeats, queue order, groups, effects function, state machine | `server/src/review_loop/**` | GH, DB | yes | ~900 |
| WP13-API (CCR) | handlers, token scope, SSE, upload validation, ADR-019 amendment | `server/src/api/review_loop.rs`, auth seam | CORE | yes | ~700 |
| WP13-WEB-C | card, group card, queue list, PR panel, table, log, waiting list | `web/src/components/review-loop/**` | A | — | ~1000 |
| WP13-WEB-S | screens, msw scenarios = mockup states, Playwright | `web/src/screens/review-loop/**`, mocks, tests | WEB-C | — | ~700 |
| WP13-E2E / SEC | full stack with fake GitHub; security review of API and uploads | `e2e/**` | all | — | ~400 |
| WP13-DOCS | brief amendment, `agent-api.md` section, runbook | docs | approval | — | docs |
Named tests (besides §7): `effects_follow_wm_matrix` (table-driven over §2.3 × author × gate × source),
`gate_thread_never_resolved_without_tick`, `repeat_by_dedupe_marker_is_not_asked`, `bot_edit_after_decision_reopens`,
`group_never_contains_gate_items`, `decision_on_changed_item_is_409`, `internal_findings_outside_exceptions_not_queued`,
`effect_report_is_idempotent`, `empty_queue_states_what_was_read`.

## 10. Risks
| ID | Risk | Mitigation |
|---|---|---|
| R13-1 | The coordinator side (claudius) is not built, so phase 1 has no data | RQ12; the owner can still track a PR and decide threads without verifications (US14) |
| R13-2 | thepastaclaw changes its markers | parser tested on recorded fixtures; unknown marker → key falls back to the thread id, a note on the PR |
| R13-3 | Two decision channels (chat and diffpilot) diverge | tokens cannot decide (T22), so chat decisions cannot be copied in by the coordinator; one channel per PR, chosen when it is tracked (RQ9) |
| R13-4 | Owner decides faster than effects run; a superseded decision already reached GitHub | effects state shown; supersede lists done effects (US11) |
| R13-5 | Verification at an old head | head shown on every verification; a verification older than the head is marked |

## 11. Open questions for the owner
| ID | Question | Linked |
|---|---|---|
| RQ1 | Stage and branch: a new version (`feat/v1.3`) after v1.2? | plan §1 |
| RQ2 | Phase 3: should diffpilot post replies and resolve threads itself, and as whom (you via the App, the App bot, or keep the coordinator's account only)? | WM A2, S13-3 |
| RQ3 | Repeats with a decided `dedupe=` key: carry the earlier decision without asking (practice since 11-21) — and what is new evidence: bot edit, new comment, diff change on that path (RR S8 + C5)? | RR M-6, R2, R3 |
| RQ4 | Gate items never grouped? | WM §6.4 item 5 |
| RQ5 | "Decide the rest per recommendation": offer it (WM §6.4 item 9), and exclude gate items? | WM §6.4 item 7 |
| RQ6 | "Accept despite the gate": is it a reject with a gate override, and does it resolve the thread? | WM §6.4 item 4 |
| RQ7 | Recommendation and verification visible on the card after the evidence (proposal), or collapsed as brief §1 does for assessments? | brief §1 |
| RQ8 | Language: questions reach you in Polish (WM §6.4 item 3); diffpilot's interface is English. Agent text in Polish, English, or your LLM-language setting? | brief §2 Language |
| RQ9 | Which PRs are tracked: on "Track" and on the first upload (proposal), or every PR of My work? While tracked, is diffpilot the only decision channel for that PR? | R13-3 |
| RQ10 | Round budget (5, WM T8) vs merge proposal after round 3 (R7): what does the PR header show, and what happens at the limit? | RR M-4, C-5, WM U3 |
| RQ11 | Standing authorization for dismissals once every bot blocker is decided (R12): if granted, does diffpilot list it as done-by-coordinator instead of waiting for you? | RR M-7 |
| RQ12 | Who builds the coordinator side (claudius `check-pr-comments` diffpilot mode vs user-level playbook), and is diffpilot's contract the source for the verify-only report (T-3c2b5f03)? | RR M-1, M-2 |
| RQ13 | Internal findings: only the three WM §6.6 exceptions enter your queue; the rest read-only. Confirm. | RR M-3, T-ca593a84 |
| RQ14 | Keep the decision log after merge or close: how long? | — |
| RQ15 | Should the coordinator report held pushes and pending `ghsudo` approvals to diffpilot (V3, V4)? | WM §15 view 4, M15 |
| RQ16 | Read thepastaclaw's status feed (F6) through a plug-in (a new outbound host)? | ADR-022 |
| RQ17 | Human and `claude`-account threads follow the human rows of §2.3; is the dashboard's `known_bots` list the one to use? | WM §1 note |
| RQ18 | Browser notification (existing setting) when new questions or answers arrive? | brief 2026-11-19 |
| RQ19 | Allow links to `http://agentic/…` reports (coverage, report.md) as an allowed origin? | T23 |

Open RR decisions without UI effect here: M-5, M-8, M-9, M-10, M-11, M-12 (they change what the coordinator and bot
produce, not this design).

## 12. Assumptions to confirm
**[A]** body-item ordinals stay stable across re-verification of the same source; **[A]** Q-L reads cost about one
point per page; **[A]** `lastEditedAt` changes on thepastaclaw's in-place edits; **[A]** a claudius report fits in 2 MiB;
**[A]** the coordinator can hold an SSE stream (otherwise polling RL9 every 120 s, WM T6).
