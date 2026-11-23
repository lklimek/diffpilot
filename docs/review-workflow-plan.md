# diffpilot — review loop plan: processing PR comments

Status: specification, 2026-11-22. Owner questions RQ1–RQ23 answered or closed (§11). Owner rule: design in
HTML first; no implementation until the owner has looked at the mockup and discussed it. Stage v1.3 on `feat/v1.3`,
cut from `main` (RQ1). Mirrors `docs/implementation-plan.md` (cited "plan §N") and `docs/v1.1-dashboard-plan.md`
("dashboard plan"): their rules (authority, git flow, standing brief block, guard A1–A12, merge checklist, CCR) apply
unchanged unless this file says otherwise. Decision: ADR-023 (proposed). Mockup: `docs/mockups/10-review-loop.html`
(this plan wins on behaviour).

Workflow source: the working model `/data/artifacts/dash-evo-tool/2026-11-22/handoff/working-model.md`, cited "WM"
with its IDs (actors A1–A9, artifacts F1–F14, loop L1–L11, timing T1–T10, measures M1–M15, gaps U1–U8);
`.../handoff/recommendations-register.md` cited "RR". Both are untrusted inputs read as data. The owner's model of
2026-11-22 (§0) overrides WM where they differ: verification moves from the coordinator into a diffpilot Codex job,
and implementing agents only consume decisions. Markers: **[A]** assumption not verified in code, docs or a spike;
**[S-n]** answered by spike S13-n (§9.1).

**Line for `CLAUDE.md` "Read first"** (coordinator adds it after approval):
`- docs/review-workflow-plan.md — stage v1.3: PR page, Comments (decisions on PR review comments, Codex assessment, decision feed for external agents); ADR-023.`

## 0. Owner model, actors, scope

**Problem.** The owner decides every comment fix per ID, one question at a time (WM §6.4), about 178 a day (M5); 40%
of blocker entries are repeats (M3); decisions and recommendations live in a chat and a log file (U1); IDs are not
stable (U2); the per-PR overview of rounds and decisions is assembled by hand.

**Owner model (2026-11-22).**
1. **Navigation.** My work → a pull request → the **PR page**, which replaces and extends today's progress page. It
   offers **Review a PR** (with the analysis status, "k of n flows analysed") and **Process PR comments**. Inside the
   review, **Comments** is one more entry of the view menu beside Flows, Files and Dependencies; Comments does not
   need a finished, or any, analysis.
2. **No tracking.** Any pull request can be processed on demand. Other users or CLI agents may process the same
   comments independently; diffpilot shows GitHub's state and records only what was decided and executed through
   diffpilot.
3. **diffpilot's Codex assesses, never implements.** A read-only runner job (`verify_comments`) checks each comment
   against the PR head and writes the assessment and recommendation (RQ20).
4. **External implementing agents** (other machines, outside diffpilot's control) subscribe with a personal access
   token, are notified of decisions they can implement, claim them and report execution. They upload nothing else.
5. **Watch.** A user marks the PRs he is working on (a per-user toggle on the PR page; My work stays unchanged; a list of
   watched PRs). For a watched PR new comments and reviews are detected automatically, the assessment runs at once
   and the user is notified that questions are ready (§2.8). Unwatched PRs stay on demand.
6. **Notifications** (RQ23): browser notifications plus per-user push channels in Settings (ntfy, generic webhook);
   an agent's token may register a push target that wakes it when a decision it can claim appears (§5.5).

| ID | Actor | Role here |
|---|---|---|
| AC1 | owner (WM A1) | the only one who decides; browser session |
| AC2 | other diffpilot users | may decide on the same PR; decisions are attributed; a stale card gets 409 |
| AC3 | diffpilot Codex (runner) | `verify_comments` jobs: assessment, recommendation, code context, answers to the owner's questions |
| AC4 | external implementing agent (e.g. WM A2 with A3) | reads the decision feed, claims decisions, executes effects (fix, push, reply, resolve, follow-up PR, TODO, dismissal), reports each; token scope `review_loop:execute` |
| AC5 | diffpilot[bot] (phase 3) | the server posts replies, PR-level comments, resolves and dismissals of owner decisions as the diffpilot GitHub App (RQ2) |
| AC6 | bots, PR Hygiene, thepastaclaw status, CI (WM A6–A9) | read by the server: GitHub, plug-ins (ADR-022) |

**In scope:** the PR page, the Comments view and its pages, Codex assessment, decisions and effects, the decision
feed, repeats and carry-overs, rounds and PR-level questions, Watch, the thepastaclaw plug-in, notifications.
**Not in scope:** internal review reports (RQ13); tracking of pushes held elsewhere and `ghsudo` (RQ15); any code
change by diffpilot; merging and the other WM §10 actions except dismissals decided here (RQ11); bot configuration
(RR R3, R4, R13); data retention (planned: one system-wide policy, 13 months, for all diffpilot data, existing data
included; out of scope now, all data kept; RQ14, RQ22).

## 1. User stories (Given / When / Then)

| ID | Actor | Story |
|---|---|---|
| US1 | AC1 | **Given** a PR row in My work, **when** the owner opens it, **then** the PR page shows "Review a PR" with the analysis state (none, planning, k of n flows analysed, paused, finished, head moved) and its actions, and "Process PR comments" with open comments, undecided questions, the round, the assessment state and "Open Comments". |
| US2 | AC1 | **Given** a PR without any analysis, **when** he opens Comments, **then** the server reads the PR's threads, reviews and comments from GitHub, starts one `verify_comments` job for unassessed items, and shows items as "LLM assessment running" until it lands; no analysis is started. |
| US3 | AC1 | **Given** assessed items, **when** he opens Comments, **then** one question is shown: the PR's purpose, the comment (GitHub tag), the discussed code with its surroundings and an LLM sentence on what that code does, facts read from GitHub, the LLM assessment, then the options with the recommended one first and marked "(Recommended)" (RQ7). |
| US4 | AC1 | **Given** a question, **when** he decides, **then** the decision is stored with the recommendation shown at that moment and its effects, the next question opens, and the feed announces the decision to subscribed agents. |
| US5 | AC1 | **Given** three non-gate items share a recommendation, **when** they come up, **then** one group question lists them with "Discuss individually" per row; each item gets its own record. Gate items never appear in a group (RQ4). |
| US6 | AC1 | **Given** a gate item (G-FUNDS), **when** he rejects it, **then** "Resolve this thread" is shown ticked; submitting it ticked is his explicit resolve, unticking it posts the reply and leaves the thread open; **when** he chooses "Accept despite the gate", **then** a reason is required, posted as the reply, and the thread is always resolved (RQ6). |
| US7 | AC1 | **Given** 7 undecided non-gate items and 2 gate items, **when** he chooses "Decide the rest per recommendation", **then** a confirmation list shows the 7 with recommendations and effects, the count, and the 2 gate items as left out; nothing is stored until he confirms (RQ5). |
| US8 | AC1 | **Given** the table page, **when** he sets a decision per row (default = recommendation; gate rows empty until chosen) and submits once, **then** one record per row is stored; submit stays disabled while a gate row has no choice or a row lacks its reason or target (RQ21). |
| US9 | AC1 | **Given** a new bot thread with the `dedupe=` marker of decided item C3, **when** the server reads the PR, **then** C3's decision is carried without a question and logged; **if** the bot edited the comment text after the decision or a commit after the decision changed the code at the thread's lines, **then** it is asked again with that reason, and the re-ask is logged (RQ3). |
| US10 | AC1 | **Given** carry-overs happened, **when** he opens Carry-overs (this PR or all PRs), **then** he sees counts (carried, asked again by reason, effects pending) and a dated list with evidence. |
| US11 | AC1 | **Given** bot round 3 has ended, **when** he opens Comments, **then** the first question is the PR-level "Merge now, remaining items as follow-ups?" with "Merge now; remaining items become follow-ups" and "Another round" (RQ10). |
| US12 | AC1 | **Given** every item of a bot's `CHANGES_REQUESTED` review is decided at the end of a round, **when** he opens Comments, **then** a PR-level question "Dismiss stale bot reviews?" lists those reviews; "Dismiss" puts a dismissal effect into the feed (RQ11). |
| US13 | AC1 | **Given** a question, **when** he presses "Ask about this comment" (`a`) and asks "why is this pre-existing?", **then** the floating Ask chat of Flows and Files opens with the subject chip "Comment C14", the answer arrives in that thread (LLM tag) with follow-ups, the card and the table row show "1 Ask answer", and the item keeps its options (WM §6.4 item 6). |
| US25 | AC1 | **Given** any decision, **when** he writes "keep the test small" in "Your comment", **then** it is stored with the decision, shown in the log and the table, sent to the executing agent with the decision, and not posted to GitHub. |
| US26 | AC1 | **Given** an analysis of the head exists, **when** a question opens, **then** the card lists the flows and steps containing the commented lines (links into Flows) and icons for notes about this code elsewhere (Flows flag, Files incidental note, Dependencies, PR Hygiene, thepastaclaw), each opening its place in a new tab; without an analysis it says "No analysis yet". |
| US27 | AC1 | **Given** the table page, **when** he clicks C14's problem, **then** Questions opens with C14; after he decides there, the table row shows that decision, his comment and the Ask indicator. |
| US14 | AC4 | **Given** a token with `review_loop:execute` and capabilities `fix, reply, resolve`, **when** the agent holds the feed stream, **then** it is notified of decisions whose open effects are within its capabilities, claims one (lease), executes the effects in order and reports each with a reference; another agent's claim on it answers 409. |
| US15 | AC4 | **Given** a `review_loop:execute` token, **when** it calls a decision route, **then** 403 `token_not_accepted`. |
| US16 | AC1 | **Given** "Notify me when work finishes" is on, **when** a round's assessment adds new questions, **then** one browser notification says so; **when** the agents report every decision of that round executed, **then** one more (RQ18). |
| US17 | AC1 | **Given** a thread was resolved on GitHub by someone outside diffpilot, **when** the PR is read, **then** the item shows "Resolved on GitHub by <login>, not through diffpilot" and leaves the queue; nothing is recorded as a decision. |
| US18 | AC1 | **Given** GitHub cannot be read, **when** Comments loads, **then** the last read is shown with its time and the failure in plain words; decisions on already read items still work. |
| US19 | AC1 | **Given** the PR page, **when** he switches Watch on, **then** the PR appears in his watched list with "Watching since 10:05" and the note that new comments are assessed automatically and use LLM tokens. |
| US20 | AC1 | **Given** a watched PR and no diffpilot page open on it, **when** the bot posts 5 new threads, **then** the next probe or plug-in signal detects them, a full read and one assessment job run (after the bot's round ends, if thepastaclaw still reviews), and a notification "acme/node-client #482: 5 questions ready" is created; the watched list and the PR page show "Assessed automatically 10:52 · 5 questions". |
| US21 | AC1 | **Given** a watched PR with an assessment job already queued or running, **when** more comments arrive, **then** no second job starts; the new items join the next pass once the current one ends and the minimum spacing has passed. |
| US22 | AC1 | **Given** Settings → Notifications, **when** he enters an ntfy server URL, topic and access token and presses "Send test", **then** a test message arrives, the token is shown afterwards only as "•••• 9f2a", and the channel shows "Last delivery: test, delivered 10:58". |
| US23 | AC1 | **Given** a webhook channel whose host is down, **when** a "questions ready" event is delivered, **then** it is tried 3 times, the channel shows "Failing since 11:02: connection refused (3 attempts)", one warning is logged, and the browser notification still appears on an open tab. |
| US24 | AC4 | **Given** an agent token registered an ntfy push target and holds no live connection, **when** a decision within its capabilities appears, **then** the topic gets one message with the decision ID, the PR and the API link to fetch it, and no reply text, reason or token. |

## 2. Workflow semantics

### 2.1 Items
| ID | Source (WM §6.1) | Read by | Stable key | Question |
|---|---|---|---|---|
| I1 | review thread (F3) | server, GitHub | the bot's `dedupe=` marker, else thread node id | yes |
| I2 | review-body item (F4: `CHANGES_REQUESTED` bodies, each human's latest review, bot body findings without a thread) | server reads; Codex splits a body into items | review id + ordinal of the split **[A]** | yes |
| I3 | human PR-level comment | as I2 | comment id + ordinal | yes |
| I4 | PR-level question (round-3 merge, stale-review dismissal) | server, from rounds | `pr:<round>:merge`, `pr:<round>:dismiss` | yes |

Bot: GitHub `Bot` type, a `[bot]` login, or Settings `known_bots`; `claude` stays human unless listed (RQ17). Each
item gets a per-PR number (`C1`, `C2`, …) at first sight, never reused. Threads resolved or answered outside
diffpilot keep their GitHub state and leave the queue when resolved (US17). An item a complete read no longer produces
(deleted on GitHub, or no longer an item by these rules) is marked `gone_at`, labelled "Not in the latest read from
GitHub" (never "resolved"), and leaves the queue, "Decide the rest" and bulk decisions; its row and decisions stay,
and a later read that produces it again clears the mark.

### 2.2 LLM assessment (`verify_comments` job, RQ20)
Per item at the PR head: validity (`valid`, `partly_valid`, `invalid`) with `file:line` evidence; pre-existing on base;
category (claudius #118 catalog, 17 slugs); necessity verdict (`in` with evidence / `out` with reason, better-fitting
PR); tripped gates (`severity` §2 IDs); elsewhere (open PR, issue, upstream); fix sketch and size (`XS`/`S`/`M`);
reply drafts; recommendation (decision kind, reason, target, reason class); **code context**: the enclosing range
(function or block) of the discussed lines and one paragraph on what that code does; and once per pass the PR's
purpose in one sentence (or the PR context of the latest analysis when one exists). Everything except IDs, SHAs and
ranges is LLM text in the user's LLM-language setting (RQ8). A missing or failed assessment leaves the item asked
without a recommendation ("Not assessed: <reason>"), never hidden.

### 2.3 Decisions and effects (WM §6.4 item 4, §6.5)
| ID | Decision | Input | Effects (executor); "→ resolve" only when "Resolve this thread" is ticked |
|---|---|---|---|
| D1 | Resolve (already fixed) | — | human thread: reply what was done (E); resolve (E), always |
| D2 | Fix here | — | fix + push (A) → reply with commit (E) → resolve (E) |
| D3 | Reject | reason | reply reason (E) → resolve (E) |
| D4 | Handled in #N (existing PR) | open PR number | reply "Per maintainer decision, this is addressed in #N." (E) → resolve (E) |
| D5 | Create follow-up PR | — (optional title, prefilled) | create a draft follow-up PR (A) → reply with its link (E) → resolve (E); no link is asked for: the PR does not exist yet |
| D6 | Upstream issue | URL, or "create" | create issue if asked (A) → reply link (E) → resolve (E); "Known limitation" body line (A) |
| D7 | TODO | title | save TODO (A) → reply "Per maintainer decision, out of scope for this PR." (E; never mentions the TODO) → resolve (E); save failed → nothing else |
| D8 | Accept despite the gate (gate items only) | reason, required | reply reason (E) → resolve (E), always |
| P1 | Merge now; remaining items become follow-ups (I4) | — | one follow-up PR listing the remaining items (A) → per item reply link (E) → resolve (E); the merge is the owner's on GitHub (link) |
| P2 | Another round (I4) | — | none |
| P3 | Dismiss stale bot reviews (I4) | message (prefilled) | dismiss each listed review (E) |
| P4 | Keep the reviews (I4) | — | none |

A = an external agent; E = an external agent in phases 1–2, diffpilot[bot] in phase 3 (§5.3).
- **Owner's comment** (every decision, D1–D8 and P1–P4, optional free text): stored with the decision, shown in the
  log and the table, delivered to the executing agent in the feed; never posted to GitHub by itself (the reply text is
  what gets posted).
- **"Resolve this thread"** is shown on every thread decision, ticked by default, neutral styling; the owner may
  untick it (reply without resolving). The submitted state is the explicit resolve decision, gate items included; D1
  and D8 keep it ticked and locked (always resolve, RQ6). Nothing is resolved without the owner's submit.
- **Override flag:** `necessity_gate` (fix although the verdict is `out`) or `blocker_gate`; D8 is stored as D3 with
  `blocker_gate(<gate>)` (RQ6).
- **Gate items** (gate from the assessment or marked by the owner; G-FUNDS, G-SECRET, G-CRYPTO, G-DATA): never
  grouped, never in "Decide the rest", and on the table page their select starts empty (RQ4, RQ5, RQ21).
- **Body and PR-comment items** have no thread: replies go into one PR-level comment per source review.
- Effects carry `seq` and `after` (a reply after a fix waits for the fix's commit); replies are English and say "per
  maintainer decision" (WM §6.5); a superseding decision cancels effects not yet claimed and lists done ones.

### 2.4 Queue, groups, "Decide the rest", table page
One question at a time, deterministic order: 1. PR-level questions (I4); 2. repeats asked again; 3. answers to the
owner's questions; 4. gate items; 5. items the reviewer marked Blocking (the bot's claim, WM §11), by first round, then
ID; 6. other single items; 7. groups. A **group** is two or more assessed non-gate items with the same recommended
decision kind and reason class or target, with one decision and "Discuss individually" per row. **Decide the rest
per recommendation** opens a confirmation list (rows untickable) with the count and the gate items named as left
out; nothing is stored before confirmation; records carry `via = bulk`. The **table page** (RQ21) shows the columns of
claudius `check-pr-comments` §4 (ID, Source, Problem, Scope, Elsewhere, Opinion, Recommendation) under one line
"PR #N: <purpose>", plus a decision select per row (default = recommendation; empty for gate rows), the reason or
target field the choice needs, "Resolve" (ticked by default), the owner's comment and an Ask indicator; one submit
stores one record per row (`via = table`); no shorthand text input. Each row's problem links to Questions with that
item open; decisions, comments and Ask answers given in Questions or the Ask chat show in the row.

### 2.5 Carry-overs (RQ3)
- **Carry:** a finding whose `dedupe=` key matches a decided item takes that decision without a question
  (`via = carried`, `carried_from`), with the same effects.
- **Asked again** only on server-read evidence: (a) the bot edited the comment text after the decision (`lastEditedAt`
  later and the body differs from the one stored at decision time); (b) a commit after the decision's head touched the
  thread's lines (diff in the server's mirror, the decided range mapped forward) **[S-4]**.
- **Owner asked again, no carry:** a human reviewer's decided thread unresolved or raised again in a new thread, or a
  carried decision whose effects no longer fit the item, is reopened (state `reopened`, queue rank 2) without a carry
  event. A re-ask or reopen holds against later reports and lease ends on the earlier decision; its unclaimed effects
  are cancelled.
- **Log and metrics:** every carry and re-ask is an event with reason and evidence; V5 shows counts and the dated list
  per PR and across all PRs. Counts only, no percentages.

### 2.6 Rounds and PR-level questions
A round is a submitted thepastaclaw review whose body carries `phase=final sha=<head>` (WM F4) **[S-1]**; the PR
header shows the round counter. When round 3 ends, and after each later round, P1/P2 is asked once (RQ10) **[A]
"each later round"**. At the end of a round, when a bot's latest review requests changes and every item it raised has
a decision, P3/P4 is asked once (RQ11).

### 2.7 States
Item: `undecided` → `asked_back` → `decided` → `executing` (effects claimed or pending) → `done` (`assessing` is
reserved: a running pass shows as the item's assessment state);
`repeat` (carried); `reopened`; `outside` (resolved or answered on GitHub outside diffpilot). Decision: append-only.

### 2.8 Watch: detection, assessment, notification
- **Watch list.** Per user (`pr_watches`), toggled on the PR page; at most 20 watched PRs per user
  **[A]**; a closed or merged PR is unwatched automatically with a note in the list.
- **Detection, reusing refresh machinery** (no webhooks):
  1. **Probe** (in the ADR-020 scheduler task, every 5 minutes **[A]**): one GraphQL request per 10 watched PRs of a
     user, by alias (the dashboard Q2 batching rule), with the user's token: `headRefOid updatedAt`,
     `reviewThreads{totalCount}`, `comments{totalCount}`, `reviews(last:1){id}` (a bot's in-place edit bumps
     `updatedAt`) **[A]**. A PR whose values differ from the stored ones gets a full read (Q-L1–Q-L4). Cost: ≈1 point per 10 PRs per
     probe, 12 per hour for 10 PRs **[A]**.
  2. **Plug-in change signals:** a changed PR Hygiene change mark, or thepastaclaw leaving `live.active` for a watched
     PR (round end), triggers the probe for that PR at once. ADR-022 amendment: plug-ins also poll while any user has
     a watched PR (not only while a page is read).
  3. **Daily scheduled refresh** (ADR-020) runs the probe for every watched PR of that user.
  The user's GitHub token pair is used as for the scheduled refresh; without it (signed out everywhere, 30-day
  bound) watching pauses and the list says "Watching paused: sign in again".
- **Assessment, bounded LLM cost.** Each detection that finds new or changed items spends LLM tokens. Bounds: one
  `verify_comments` job per PR queued or running; while the thepastaclaw plug-in reports the PR's head as reviewing or
  queued, the pass waits for the round's end (at most 90 minutes **[A]**) so one round is assessed once; at least 10
  minutes between passes on one PR **[A]**; fingerprint reuse (§4.2); the daily per-user cap shared with on-demand
  passes; when the cap is reached the list says "Not assessed automatically: today's limit" and the PR stays on demand.
- **Notification.** A pass that adds questions creates one per-user event "acme/node-client #482: 5 questions ready"
  (one per round, RQ18), delivered to open tabs as a browser notification and to the user's push channels (§5.5).
- **Display.** Watched list (`/watching`): PR, watching since, last probe, last
  automatic assessment ("Assessed automatically 10:52 · 5 questions"), state (waiting for the bot's round, job
  running, limit reached, paused), Unwatch. The PR page shows the Watch toggle and the same badge.

## 3. Views and flows

Global: brief §4 rules 1–3 (full width, tooltips on every label), loading / empty / error states, keyboard, WCAG 2.2
AA (§3.8). The Comments pages share the review top bar: breadcrumb "diffpilot › owner/repo #N [GitHub] › view menu
(Flows / Files / Dependencies / Comments)"; without an analysis the menu offers Comments only, the other entries
read "Needs an analysis" and link to the PR page.

| ID | View | Route (web) | WM §15 | Phase |
|---|---|---|---|---|
| V0 | PR page: Review a PR + Process PR comments | `/pr/:owner/:repo/:n` (replaces `/analyses/:id/progress`, which redirects) | — | 1 |
| V1 | Questions (one at a time), group, "Decide the rest", PR-level questions; `?item=C14` opens that item | `/pr/:owner/:repo/:n/comments` | view 1 | 1 |
| V2 | Table page | `…/comments/table` | view 2 | 1 |
| V3 | Decision log and execution (feed state, agent reports) | `…/comments/log` | U1 | 1 |
| V4 | Rounds and per-item history | `…/comments/rounds` | view 5 | 1 (rounds), 2 (history panel) |
| V5 | Carry-overs, this PR / all PRs | `…/comments/carry-overs`, `/comments/carry-overs` | — | 1 |
| V6 | Watched PRs (list), Watch toggle on V0 | `/watching` | view 3 (partly) | 1 |
| V7 | Settings → Notifications: browser toggle, ntfy, webhook, per-event toggles, Send test, last delivery | `/settings#notifications` | — | 1 |
My work stays unchanged (owner 2026-11-22): no chips, toggles or badges there; the cross-PR view (WM view 3) is the watched list V6. WM view 4 (push
state) is out of scope (RQ15).

### 3.1 V0 PR page
Header: title, `#N`, base (stack parent when the base is another PR's branch), draft, head, CI, PR Hygiene note,
thepastaclaw status, the Watch toggle with its badge. Two cards side by side:
- **Review a PR:** today's progress content (state, "k of n flows analysed", current stage, estimate, Pause/Resume,
  Cancel, "Analyse all remaining flows", Open) or "No analysis yet · Check and start"; the language tag.
- **Process PR comments:** unresolved threads on GitHub (fact), undecided questions, waiting for agents, round N,
  assessment state ("LLM assessment running: 4 of 9 comments"), "Open Comments" and "Table".

### 3.2 V1 Questions: the card (top to bottom: context, evidence, assessment, recommendation, decision)
1. Header: `C14 · 2 of 9`; source and author ("thepastaclaw · bot · inline thread"); first seen (round, head); the
   bot's severity marker as a GitHub-tagged claim; gate chip; category; repeat-of link; **notes elsewhere**: icons only,
   each with an accessible name and a tooltip on hover and focus ("Flows: the LLM flagged these lines (1 flag)",
   "Files: incidental-change note", "Dependencies: 3 changed callers", "PR Hygiene: 2 blockers", "thepastaclaw:
   reviewing"), each opening its place (step, line, node, plug-in home) in a new tab; absent sources show no icon.
   **In flows:** the flows and steps whose ranges contain the commented lines, from the analysis of this head, as
   links into Flows ("Flow 4 · Retry on timeout · step 2"); an older-head analysis is labelled with its head; none →
   "No analysis yet · Start on the PR page".
2. **PR context:** "PR #482: <purpose>" (LLM tag, or the analysis's PR context).
3. **Comment** (GitHub tag; plain text; links only `https://github.com/…`).
4. **Code:** the enclosing range at the head with the commented lines marked, collapsed beyond 40 lines, "Show more
   above / below"; under it "What this code does" (LLM tag); "Open in Files" when an analysis of this head exists.
5. **Facts** read by diffpilot (badges name the check): thread unresolved / outdated, edited after a decision, marker.
6. **LLM assessment** (LLM tag, "diffpilot Codex at 9f3c2e1, 10:32"): validity and evidence, pre-existing, necessity,
   gates, elsewhere, fix sketch and size, earlier decisions.
7. **Your decision:** radio group, recommended first with "(Recommended)" (RQ7); D1–D7 ("Create follow-up PR" for D5,
   "Handled in another PR" for D4), D8 on gate items; inputs per choice; override select; "Resolve this thread"
   (ticked by default, neutral; locked ticked for D1 and D8); **Your comment (optional)** with the hint "Saved with
   the decision and sent to the agent that executes it. Not posted to GitHub: the reply text below is what gets
   posted."
8. **What happens next:** the effects in order with their executor ("External agent", "diffpilot[bot]" in phase 3) and
   the reply preview with its author and, in phase 3, the footer "Posted by diffpilot on behalf of the maintainer's
   decision (C14)."
9. Actions: **Decide** (Enter), **Ask about this comment** (`a`; the shared Ask button and floating Ask chat of Flows
   and Files, brief amendment 2026-11-19, subject chip "Comment C14"; no separate ask box), **Skip for now**. After
   Decide the next card opens with focus on its heading; the live region announces the outcome and the count; Undo
   until an effect is claimed. The Ask chat's threads about this item are counted on the card ("1 Ask answer").

**Empty:** "No undecided questions in what was read at 10:41: 14 threads (all pages read), 4 reviews, 9 comments
assessed at 9f3c2e1." **Error:** "GitHub could not be read (rate limit). Showing the read of 10:12; next read at 10:52."

### 3.3 V2 Table page
"PR #482: <purpose>" line; columns of `check-pr-comments` §4 plus Decision (select), Reason / target, Resolve
(checkbox, ticked), Your comment and Ask (count of answers, opens the chat thread); the Problem cell links to
Questions with that item open (`?item=`); sections Undecided and Repeats asked again; decided, carried and outside
items below as read-only rows showing decision, comment and where it was decided (Questions, group, table, carried). Live summary "Will
record 9 decisions: Fix here 3 · Reject 4 · Follow-up PR 1 · Accept despite the gate 1"; Submit disabled with the
reason while a gate row is empty or a field is missing. Real `<table>`; selects labelled by the row's ID and problem.

### 3.4 V3 Decision log and execution
Chronological: decision, item, by, at, recommendation then, `via`, override, effects with executor, claim (agent token
name, lease), reports (done with reference, failed with reason), and GitHub facts ("Reply found on GitHub · 10:46").
Header: agents subscribed now (token names, capabilities, last seen), open effects, failed effects.

### 3.5 V4 Rounds
Per round: head, finished, bot verdict, new, still open, carried, asked again, decisions by kind, undecided (counts).
Phase 2: per-item history panel (every round the key appeared in, every decision).

### 3.6 V5 Carry-overs
Scope switch "This pull request / All pull requests"; tiles (carried, asked again: bot edited, asked again: code
changed, carried effects pending); dated list (item, repeat of, decision carried, event, why with evidence link,
effects, Reopen). All PRs: one row per PR with the same counts.

### 3.7 Wording (brief §1; the wording checks cover `components/comments/**`, `screens/pr/**`, `screens/comments/**`)
LLM text: "LLM" tag; tooltip "written by diffpilot's LLM, not checked". Validity reads "The LLM says: valid / partly
valid / invalid". Section title "LLM assessment", never "verification" or "verified". A bot's severity is its own
words with the "GitHub" tag. No numeric confidence; counts, never percentages. External agents' reports read "Agent
<token name> reports: replied (link)"; a GitHub read adds a fact badge. Plug-in statements are attributed
("thepastaclaw status: reviewing (verify, 3/5) · data of 10:21").

### 3.8 Keyboard and accessibility
Keys: `j`/`k` next/previous question, `1`–`8` choose, `Enter` decide, `i` discuss individually, `a` Ask, `s` skip,
`g` GitHub, `t` table, `?` help, `Esc`. Card heading focused on load; decisions are a `radiogroup` whose accessible
names include "(Recommended)"; gate checkbox described by its rule; live region for outcomes; tables with headers;
targets ≥ 24 px; focus not hidden by sticky bars; no colour-only meaning; axe on every state, both themes.

### 3.9 Collapsible sections and state badges
- **GitHub link-outs** (owner 2026-11-22): every element with a GitHub counterpart (PR title, comment, thread, reply,
  review, commit, round) carries the GitHub mark link-out used in My work (same component, new tab).
- **Collapsible sections:** every section heading (h2/h3 of a page or card, not the question title) is a disclosure
  button with a chevron (`aria-expanded`, `aria-controls`, Enter/Space); open by default; the state is remembered
  per view and user in local storage. One shared component (`CollapsibleSection`).
- **State badges** (one per PR, on the watched list, the PR page and the Comments header), each colour
  paired with an icon and a text label, contrast ≥ 4.5:1 in both themes; first match wins:

| ID | Label | Colour (token) | Rule (facts diffpilot read; tooltip lists them) |
|---|---|---|---|
| B1 | Blocked / failing | red (`high`) | a failed effect, CI failed (the reader's Ignored checks, failing or pending, left out as in My work), or a push channel failing for this PR's events |
| B2 | Questions waiting | amber (`med`) | undecided questions exist |
| B3 | Executing | teal (`bot`) | decided effects are open or claimed |
| B4 | Waiting for bot round | blue (`low`) | thepastaclaw reports this head reviewing or queued, or a watched pass waits for the round |
| B5 | Ready to merge | green (`ok`) | none of B1–B4; CI passed; no review requests changes (or all such are dismissed); PR Hygiene, when present, agrees; My work lists it as Ready to merge (K17) at this head |
  Unknown inputs (CI not read, GitHub read failed, My work's last scan did not read this head) show no B5; the
  tooltip says what is missing.

## 4. Data and sources

### 4.1 GitHub reads (server, the reader's user token; queries only)
Q-L1 PR facts (`title state isDraft baseRefName headRefOid reviewDecision`); Q-L2 `reviewThreads` with all comments
(`databaseId author{login __typename} body createdAt lastEditedAt url pullRequestReview{id}`), cap 1000 threads;
Q-L3 `reviews` (`id author state submittedAt commit{oid} body`); Q-L4 PR comments. Read when Comments or the PR page
opens and the last read is older than 60 s, every 60 s while a Comments page is visible, on "Read GitHub again", and
after an agent report (debounced 10 s). Markers (`dedupe=`, `finding=`, severity, `phase=final sha=`) are parsed
deterministically **[S-1]**. File content for the code context comes from the server's mirror at the head (fetched on
demand), served by range (brief §7).

### 4.2 Codex job `verify_comments` (runner, read-only)
- **Contracts (CCR):** `JobKind::VerifyComments`; `JobTarget::PullRequestComments { pull_request_id, repo:
  RepoCheckout, pass_id }` (ADR-018 pattern); schema `schemas/agent/verify_comments.schema.json` (Codex strict subset,
  re-validated by the server); prompt `prompts/verify_comments.md` with a golden test and prompt version.
- **Runner:** fetch and check out the head, run the agent with the job's read-only, PR-scoped installation token for
  `github-mcp-server` (to look up "elsewhere"); no session restore or save (no third session kind; ADR-004 unchanged);
  no MemCan endpoint; same uid, egress and scrubbing rules (plan §5.4 T1–T9). It never writes to GitHub.
- **Input** (context pack, fenced as data): PR title and description, the PR context of the latest analysis if any,
  the project profile from the base ref if present, items (comment text, location, author kind), earlier decisions on
  this PR, and for a question mode the item and the owner's question.
- **Batching and cost:** ≤ 10 items per job **[A]**; an item is re-assessed only when its comment text, its thread's
  code lines or the PR purpose changed (fingerprint; otherwise the stored assessment is reused and shown with "code
  at these lines unchanged since 3f9a1c2"); a daily cap per user as a code constant, the remainder shown "Not assessed
  yet: daily limit".
- **Triggers:** opening Comments or the PR page with unassessed items; new comments or a moved head found by a read;
  "Assess again" on an item; an owner question.
- **Priority and model:** claimed after `ask` and before dashboard ranking and batch jobs; `select_model` arm
  `VerifyComments` = the installation model and effort (no new setting).

### 4.3 Tables (one migration `NNNN_review_loop.sql`; every table has `tenant_id`; PR data tenant-shared like analyses)
| ID | Table | Key columns |
|---|---|---|
| T-L1 | `comment_prs` | id, repo, number, last_read_at, read_state, `github_snapshot` JSON, round_count |
| T-L2 | `comment_items` | id, pr_id, display_no, kind, stable_key, author, author_kind, path, lines, title, url, body_hash, first_seen_head, first_round, bot_severity, gate_marks JSON, state, revision, outside JSON, gone_at |
| T-L3 | `comment_assessments` | id, item_id, job_id, head, fingerprint, fields of §2.2, recommendation JSON, prompt_version |
| T-L4 | Ask chat storage (existing) | subject kind gains `comment_item` (item id); without an analysis the chat is keyed by (PR, user) **[A]** (CCR) |
| T-L5 | `comment_decisions` | id, item_id, user_id, kind, reason, target, override, resolve_ticked, reply_text, comment, recommendation JSON, via, carried_from, supersedes, head, body_hash, decided_at (append-only) |
| T-L6 | `comment_effects` | id, decision_id, seq, after, kind, executor, params JSON, state, claim_token_id, lease_until, reference, message, updated_at |
| T-L7 | `comment_carry_events` | id, pr_id, item_id, kind (`carried`, `asked_again`), source_decision_id, reason (`bot_edited`, `code_changed`), evidence JSON, at |
| T-L8 | `agent_subscriptions` | token_id, capabilities JSON, last_seen_at, push_kind, push_url, push_topic, push_secret (AES-GCM), last_delivery_at, last_error, failing_since |
| T-L9 | `pr_watches` | user_id, repo, number, since, probe JSON (last values), last_probe_at, last_auto_pass_at, state |
| T-L10 | `user_events` | id, user_id, kind (`questions_ready`, `round_executed`, `test`), pr, count, at |
| T-L11 | `notification_channels` | id, user_id, kind (`ntfy`, `webhook`), url, topic, secret (AES-GCM), events JSON, enabled, last_delivery_at, last_error, failing_since |
| T-L12 | `notification_deliveries` | id, user_event_id or decision_id, channel_id or token_id, attempt, state, http_status, error, at (kept; the system-wide retention policy, planned, applies later) |
Retention: none now; the planned system-wide 13-month policy will cover these tables (RQ14, RQ22).

### 4.4 Coverage of WM §15
PR → T-L1 + V0 header; Round → Q-L3 markers, V4; Finding → T-L2 + T-L3; Decision → T-L5; Action → T-L6 (TODO id
stays in the agent's report, never on GitHub); Gate state → the thepastaclaw plug-in (reviewing / queued); Internal
review → out of scope (RQ13); views 1, 2, 5 → V1, V2, V4; view 3 → watched list V6; view 4 → out of scope.

## 5. How decisions reach the agents that act

### 5.1 Who does what
| Work | Who |
|---|---|
| assessment, recommendation, code context, answers | diffpilot Codex (`verify_comments`), read-only |
| decisions | owner (and other signed-in users), browser session only |
| fix, push, follow-up PR, upstream issue, TODO, PR-body edit | external agent (A) |
| reply, PR-level comment, resolve, review dismissal | external agent (phases 1–2); diffpilot[bot] (phase 3, RQ2) |
| merge, `/self-reviewed`, `/skip-bots`, retarget, `ghsudo` approvals | owner on GitHub; never diffpilot or the feed |

### 5.2 Decision feed (external agents)
1. Create a personal access token with scopes `read` and `review_loop:execute` (Settings).
2. Subscribe: `GET /api/v1/decision-feed/events` (SSE; `decision_available`, `decision_cancelled`, `heartbeat`) with
   `capabilities=fix,reply,resolve,pr_comment,follow_up_pr,upstream_issue,todo,dismiss_review` and optional
   `repo`; or poll `GET /api/v1/decision-feed` with `ETag`.
3. Claim a decision (`POST …/decisions/{id}/claim`; lease 30 min, renewed by each report; another claim → 409
   `claimed`); only decisions whose open effects all fall within the claimer's capabilities can be claimed.
   The decision carries the owner's comment (instructions for the executor, never posted by diffpilot).
4. Execute effects in `seq` order; report each (`POST …/effects/{id}/reports`: `done` + reference, `failed` + reason,
   or `released`; idempotency key). An expired lease returns the decision to the feed; a duplicate report is a no-op.
5. Effects run by diffpilot[bot] (phase 3) never appear in the feed; an agent's `fix` report with its commit releases
   the dependent reply to diffpilot[bot].
Agents upload no assessments or reports of their own (RQ12, RQ20).

### 5.3 Phase 3: diffpilot[bot] posts (RQ2)
The GitHub App gains `pull_requests: write` (each installation owner accepts it). For each due effect of a stored
decision the server mints a write installation token for the PR repository only, posts (GraphQL
`addPullRequestReviewThreadReply`, `resolveReviewThread`, `addComment`, `dismissPullRequestReview`), revokes it,
serialised per PR like ADR-006 and idempotent per effect. Every posted text ends with the footer "Posted by diffpilot on
behalf of the maintainer's decision (C14)." The token never enters a `JobSpec` or a response; the runner's token stays
read-only (ADR-005 amendment). Until the permission is accepted these effects stay with agents and the PR page says so.

### 5.4 Owner-only
Every decision, supersede, reopen, gate mark and clear, "Resolve this thread", PR-level questions. Tokens get 403
`token_not_accepted` (contract test over `ROUTES`).

### 5.5 Notifications and push targets (RQ23)
- **Events (minimal):** user events `questions_ready` (a pass added questions on a PR the user watches),
  `round_executed` (agents reported every decision of a round executed), `test`; agent event `decision_available` (a
  decision within the target's capabilities became claimable).
- **User channels** (Settings → Notifications, per user): (a) **browser notification**, the existing toggle, open
  tabs only (RW3); (b) **ntfy**, first-class: server URL (default `https://ntfy.sh`), topic, optional access token;
  message title "diffpilot", body "acme/node-client #482: 5 questions ready", `Click` header = the PR's Comments URL;
  (c) **generic webhook** (e.g. Home Assistant): URL, optional `Authorization` header value; `POST` JSON
  `{v: 1, event, text, url, pr, at}`. Per channel: on/off and which events.
- **Agent push target** (optional, per access token with `review_loop:execute`, RF5): ntfy (server, topic, token) or
  webhook URL; payload `{v: 1, event: "decision_available", decision_id, pr, fetch: "/api/v1/decision-feed/decisions/
  <id>"}`; never reply text, reasons, comment bodies or secrets. One message per claimable decision; not sent while the
  token holds a live SSE connection.
- **Secrets:** channel and push-target tokens are stored AES-GCM under `SR_SECRET_KEY` like GitHub user tokens
  (`server::secrets`); responses show only the last 4 characters; never logged; never in a `JobSpec` or the runner.
- **Delivery:** a server task per event and channel; timeout 10 s; 3 attempts (now, +1 min, +5 min) **[A]**; outcome
  stored per channel (last delivery, last error, failing since) and shown in Settings (user channels) and V3 (agent
  targets); `warn` logged once when a channel turns failing and `info` once when it recovers, not per attempt.
- **Outbound limits** (hosts are set by the user or an agent token; proportionate, not a full SSRF filter):
  `http`/`https` only; redirects not followed (a 3xx is a failure naming the target host); response bodies read up to
  4 KiB and never shown; 30 messages per hour per channel **[A]** (counted from stored sends, per user and kind or
  per token); loopback, link-local and reserved addresses always refused (in the URL and after resolution);
  private addresses allowed by default (Home Assistant and self-hosted ntfy live on the LAN), blocked when the
  operator sets `SR_NOTIFY_DENY_PRIVATE=1`; a kept secret only while kind and URL origin stay; the runbook lists
  notification hosts as operator-chosen outbound destinations.

## 6. Contracts and routes
Types in `crates/contracts/src/api/comments.rs` (`PrPage`, `CommentsOverview`, `CommentItem`, `Assessment`,
`CodeContext`, `Recommendation`, `DecisionKind`, `Override`, `Decision`, `Effect`, `EffectKind`, `Claim`,
`EffectReport`, `CarryEvent`, `Round`, `FeedEvent`), agent output in `crates/contracts/src/agent/verify_comments.rs`.
Integers only on the wire; open enums (tolerant readers); JSON Schemas for read responses in `schemas/api/`.
`{pr}` = `{owner}/{repo}/{number}`.

| ID | Method | Path (`/api/v1…`) | Auth | Request → Response |
|---|---|---|---|---|
| RC1 | GET | `/prs/{pr}` | read | → `PrPage` (analysis summary, comments summary, plug-in notes) |
| RC2 | GET | `/prs/{pr}/comments` | read | → `CommentsOverview` (header, counts, rounds, assessment state, queue order) |
| RC3 | POST | `/prs/{pr}/comments/read` | session_csrf | → 202 (read GitHub again; 429 under 10 s) |
| RC4 | GET | `/prs/{pr}/comments/items` | read | `state`, `kind` → `CommentItem[]` with assessments, decisions, effects |
| RC5 | GET | `/prs/{pr}/comments/items/{item}/context` | read | `from`, `to` → `CodeContext` (lines at head, marked range, LLM description, flows and steps containing the range, notes elsewhere with their links) |
| RC6 | POST | `/prs/{pr}/comments/items/{item}/assess` | session_csrf | → 202 (assess again) |
| RC7 | — | existing Ask chat routes | session_csrf | subject `{kind: comment_item, item}` (CCR); with an analysis of the head the Ask session answers, without one a `verify_comments` question pass answers into the same chat **[A]** |
| RC8 | POST | `/prs/{pr}/comments/items/{item}/decisions` | session_csrf | `{kind, reason?, target?, override?, resolve_ticked, reply_text?, comment?, item_revision}` → 201; 409 `item_changed` |
| RC9 | POST | `/prs/{pr}/comments/decision-batches` | session_csrf | `{via: group | bulk | table, rows[…]}` (each row with `resolve_ticked`, `comment?`) → 201 `Decision[]`; 422 for a gate row under `bulk`/`group` or an unchosen gate row under `table`; 409 lists changed items |
| RC10 | GET | `/prs/{pr}/comments/log` | read | → decisions, effects, claims, reports, carry events |
| RC11 | GET | `/comments/carry-overs` | read | `pr?`, `cursor` → events + counts (per PR and total) |
| RW1 | PUT / DELETE | `/prs/{pr}/watch` | session_csrf | → 204 (watch / unwatch; 409 `watch_limit_reached`) |
| RW2 | GET | `/watches` | read | → watched PRs with probe and automatic-assessment state |
| RW3 | GET | `/events` | read | SSE per user: `questions_ready`, `round_executed` (browser notifications) |
| RN1 | GET | `/settings/notifications` | session | → browser toggle, channels (secrets masked), last delivery per channel |
| RN2 | PUT / DELETE | `/settings/notifications/channels/{kind}` | session_csrf | `{url, topic?, secret?, events, enabled}` → 200 (masked); 422 `scheme_not_allowed`; an absent `secret` keeps the stored one |
| RN3 | POST | `/settings/notifications/channels/{kind}/test` | session_csrf | → 200 `{outcome, http_status?, error?}` (one attempt, no retry) |
| RF5 | PUT / DELETE | `/decision-feed/push-target` | `review_loop:execute` | `{kind, url, topic?, secret?}` → 200 (masked); the calling token's own target only |
| RF1 | GET | `/decision-feed` | read | `capabilities`, `repo?`, `cursor` → open decisions with effects; `ETag` |
| RF2 | GET | `/decision-feed/events` | read | SSE `FeedEvent`; registers the subscription (T-L8) |
| RF3 | POST | `/decision-feed/decisions/{id}/claim` | `review_loop:execute` | `{capabilities}` → 201 `Claim`; 409 `claimed` |
| RF4 | POST | `/decision-feed/effects/{id}/reports` | `review_loop:execute` | `{outcome, reference?, message?, idempotency_key}` → 201 |
`RouteAuth` gains `TokenOnly(TokenScope::ReviewLoopExecute)` for RF3–RF5; `TokenScope` gains `review_loop_execute`.
Error codes: `item_changed`, `claimed`, `lease_expired`, `capability_missing`. ADR-019 amendment: a token may call a
mutating route only when the route names its write scope.

## 7. Threat model additions
| ID | Control | Test |
|---|---|---|
| T22 | Decisions, gate marks and PR-level questions only with session + CSRF of an allow-listed user | `no_token_reaches_decision_routes`, `gate_resolve_needs_explicit_choice` |
| T23 | `verify_comments` holds only the read-only PR-scoped token; comment text fenced as data; output closed sets re-validated; text-only rendering | `verify_job_spec_has_read_only_token`, `verify_output_outside_closed_sets_is_rejected` |
| T24 | Comment bodies are untrusted (G-PHISH): plain text, links only `https://github.com/…`; markers parse known keys only | `markers_parse_only_known_keys`, Playwright `comment_texts_render_as_text` |
| T25 | Execute tokens: claim and report only, own claims only, audit line, rate limit (ADR-019) | `report_on_foreign_claim_is_rejected`, `execute_token_cannot_decide` |
| T26 (phase 3) | Write installation token: server-only, PR repository, per effect, revoked; only effects of stored decisions | `write_token_never_in_job_spec`, `bot_posts_only_decided_effects` |
| T27 | Watch probes are reads with the watcher's own token; the token pair is kept only within ADR-020's 30-day bound; watches pause without it | `watch_probe_sends_no_mutation`, `watch_pauses_without_token_pair` |
| T28 | Channel and push-target secrets: AES-GCM at rest, masked in every response, never logged, never in a `JobSpec`; push payloads carry IDs and links only | `channel_secret_is_masked_in_responses`, `push_payload_has_no_secret_or_text`, `job_spec_has_no_channel_secret` |
| T29 | Outbound notifications: http/https only, no redirects, 10 s timeout, response read ≤ 4 KiB, per-channel rate, optional private-address deny | `notify_rejects_non_http_scheme`, `notify_does_not_follow_redirects`, `notify_denies_private_when_configured` |
| AR-11 | Accepted (proposal): a prompt-injected comment can skew its own LLM assessment; the owner decides with the evidence shown | — |

## 8. Reuse, supersede, integrate
| Piece | Use |
|---|---|
| progress screen | its content moves into V0's "Review a PR" card; `/analyses/:id/progress` redirects to V0 |
| review top bar and view menu | gains "Comments"; works without an analysis (menu entries that need one say so) |
| claudius `triage-findings` | superseded for PR comments; kept for local review reports without a PR |
| claudius `check-pr-comments` | §4 columns for the table page; §8 matrix extended by WM §6.5 into §2.3; implementing agents may use its scripts to execute feed effects |
| dashboard, `known_bots` | entry point (unchanged); bot rule |
| PR Hygiene plug-in, plug-in host (ADR-022) | header note; second plug-in thepastaclaw status (RQ16, ADR-022 addendum below) |
| access tokens (ADR-019) | agent auth; one write scope |
| `github::threads`, mirror, lines API | extended reads; code context by range without an analysis |
| Ask chat (floating, brief amendment 2026-11-19), `AskButton` | the card's "Ask about this comment"; subject kind `comment_item`; without an analysis a `verify_comments` question pass answers into the same chat |
| notifications | browser notification setting kept, channels added (§5.5) |
| Flows ranges, incidental notes, deps graph, plug-in statements | the card's "In flows" list and notes-elsewhere icons (read only, links in new tabs) |
| `server::secrets` (AES-GCM) | channel and push-target tokens |

**thepastaclaw plug-in (RQ16).** `server::dashboard::plugin::thepastaclaw`, config `SR_THEPASTACLAW_STATUS_URL`
(default unset = off), polls `https://thepastaclaw.github.io/review-system/data/status.json` under ADR-022's rules
(60 s, only while a user reads a dashboard, PR or Comments page). `live.active[]` (repo, number, sha, started_at,
phase, progress) → "thepastaclaw: reviewing (<phase>, <progress>) since <started_at>"; `live.queued[]` (reason,
eligible_at) → "thepastaclaw: queued (<reason>, eligible <eligible_at>)"; `data_as_of` = `as_of` (trails up to about
20 minutes, within the 1-hour freshness bound); no Ready to merge opinion; a PR absent from both lists gets no
statement.

## 9. Phasing and work packages
**Phase 1 (smallest useful):** V0, V1, V2, V3, V4 rounds, V5, V6; `verify_comments`; decision feed RF1–RF4;
carry-overs; PR-level questions; Watch with probe and plug-in signals; thepastaclaw plug-in; notifications (browser,
ntfy, webhook) and agent push targets: Watch is of little use without a channel that reaches a closed tab, and a
woken agent shortens the loop. **Phase 2:** item history panel. **Phase 3:** diffpilot[bot] executes
reply, comment, resolve and dismissal effects (RQ2).

### 9.1 Spikes
| ID | Question | Gates |
|---|---|---|
| S13-1 | Recorded thepastaclaw threads, reviews, gate comments: marker grammar, in-place edits, final-phase marker; fixtures | markers, rounds |
| S13-2 | User-token cost of Q-L1–Q-L4 on the largest PR | WP13-GH |
| S13-3 | `verify_comments` prompt on 3 recorded PRs at the installation effort: agreement with the owner's recorded decisions, tokens, runtime | WP13-JOB |
| S13-4 | Mapping a decided line range forward through later commits (code-changed rule) | carry-overs |
| S13-5 (phase 3) | Installation token with `pull_requests: write`: reply, resolve, dismiss; shown identity | phase 3 |

### 9.2 Work packages (prefix WP13; sizes in lines)
| WP | Goal | Depends | Sec | Size |
|---|---|---|---|---|
| WP13-A | contracts, routes, migration, `docs/contracts/*`, T22–T25 | approval, S13-1 | yes | ~900 |
| WP13-GH | Q-L1–Q-L4, markers, fake-github additions | A, S13-2 | yes | ~600 |
| WP13-JOB (CCR) | `JobKind::VerifyComments`, `JobTarget`, runner path, `select_model` arm, schema, prompt + golden, fake provider scripts | A, S13-3 | yes | ~900 |
| WP13-CORE | items, keys, assessment reuse, queue, groups, effects, carry-overs, rounds, PR-level questions | GH, DB | yes | ~1000 |
| WP13-DB | repositories, revision checks, append-only decisions, claims and leases | A | — | ~700 |
| WP13-FEED (CCR) | RF1–RF4, SSE, execute scope, leases, ADR-019 amendment | CORE | yes | ~600 |
| WP13-PLUG | thepastaclaw plug-in; plug-in polling while watches exist (ADR-022 amendment) | — | — | ~300 |
| WP13-WATCH (CCR) | watches, probe in the ADR-020 scheduler, plug-in triggers, debounce and spacing, user events, RW1–RW3, token-pair rule (ADR-020 amendment) | CORE, PLUG | yes | ~700 |
| WP13-NOTIFY (CCR) | channels (ntfy, webhook), delivery task with retries and state, agent push targets, RN1–RN3, RF5, Settings section, runbook outbound hosts | WATCH, FEED | yes | ~650 |
| WP13-WEB-C / WEB-S | components and screens V0–V5, view-menu entry, progress redirect, msw states = mockup states, Playwright | A | — | ~1800 |
| WP13-E2E / SEC / DOCS | full stack; security review; brief amendment, `agent-api.md` feed section, runbook | all | — | ~500 + docs |
Named tests (besides §7): `effects_follow_matrix`, `gate_items_never_grouped_or_bulk`,
`accept_despite_gate_requires_reason_and_resolves`, `table_submit_blocked_until_gate_rows_chosen`,
`repeat_by_dedupe_marker_is_carried_and_logged`, `bot_edit_or_code_change_reasks`, `unchanged_item_reuses_assessment`,
`comments_work_without_analysis`, `claim_conflict_is_409`, `expired_lease_returns_decision`,
`round_three_asks_merge_question_once`, `stale_review_dismissal_question_after_round`, `outside_resolution_not_recorded`,
`watch_probe_batches_ten_prs`, `unchanged_probe_reads_nothing_more`, `one_assessment_job_per_pr`,
`watched_pass_waits_for_bot_round`, `questions_ready_event_once_per_round`, `daily_cap_stops_automatic_passes`,
`ntfy_message_has_title_body_and_click_link`, `webhook_payload_has_fixed_shape`, `delivery_tries_three_times_then_fails`,
`failure_logged_once_per_state_change`, `agent_with_live_stream_is_not_pushed`, `push_only_for_claimable_capabilities`,
`resolve_ticked_by_default_and_submitted_state_wins`, `owner_comment_in_feed_never_posted`,
`follow_up_pr_needs_no_link`, `state_badge_precedence_and_unknown_inputs`, Playwright `table_row_opens_question`,
`sections_collapse_by_keyboard`, `ask_about_comment_opens_shared_chat`, `notes_icons_open_new_tab`.

## 10. Risks
| ID | Risk | Mitigation |
|---|---|---|
| R13-1 | No external agent subscribes; decisions pile up | V3 shows "no agent subscribed"; effects stay open; phase 3 covers replies and resolves |
| R13-2 | thepastaclaw changes its markers | fixtures; unknown marker → thread id key, a note on the PR |
| R13-3 | Two agents act on one decision | claim with lease; reports only by the claimer; idempotent effects |
| R13-4 | Codex assessment cost on PRs with many comments | fingerprint reuse, batch cap, daily cap |
| R13-5 | Decisions and actions outside diffpilot diverge from ours | GitHub state shown beside ours; outside resolutions leave the queue unrecorded |
| R13-6 | Watching many busy PRs burns LLM tokens and GitHub points | watch limit, batched probe, one job per PR, bot-round debounce, spacing, daily cap; costs shown in the watched list |
| R13-7 | A push channel fails silently and the owner misses questions | per-channel failing state in Settings, warning logged once, browser notification independent |

## 11. Owner questions (all answered or closed, 2026-11-22)
| ID | Question | Answer |
|---|---|---|
| RQ1 | Stage and branch | `feat/v1.3` from `main` |
| RQ2 | Should diffpilot post replies and resolve, as whom | phase 3, as the diffpilot GitHub App bot, clearly labelled; App needs `pull_requests: write` |
| RQ3 | Repeats | carry the earlier decision without asking; ask again when the bot edited the comment text or the code at that place changed; visible log and metrics, per PR and overall |
| RQ4 | Gate items in groups | never grouped |
| RQ5 | "Decide the rest per recommendation" | offered; excludes gate items; confirmation list with the count first |
| RQ6 | "Accept despite the gate" | reject with a gate override; reason required and posted; thread always resolved |
| RQ7 | Recommendation on the card | visible under the evidence, recommended option first "(Recommended)"; plus PR purpose and the surrounding code |
| RQ8 | Language of agent text | the existing LLM-language setting |
| RQ9 | Tracking, single channel | no tracking; any PR on demand; diffpilot is not the only channel |
| RQ10 | Round budget | round counter in the PR header; after round 3 the PR-level question "Merge now, remaining items as follow-ups?" |
| RQ11 | Dismissals | at the end of a round the PR-level question "Dismiss stale bot reviews?"; executed from the feed (agent, or diffpilot[bot] in phase 3) |
| RQ12 | Coordinator-side contract | closed: diffpilot's API is the only contract |
| RQ13 | Internal review findings | closed: out of scope |
| RQ14 | Log retention | planned system-wide 13-month policy; out of scope now; keep all data |
| RQ15 | Held pushes, `ghsudo` | closed: not tracked |
| RQ16 | thepastaclaw status | second plug-in (§8), shown in the PR header and Comments, attributed |
| RQ17 | Bot rule | `Bot` type, `[bot]` login or `known_bots`; `claude` human unless listed |
| RQ18 | Notifications | existing setting; one per round when new questions appear after assessment, one when agents report the round's decisions executed |
| RQ19 | Report links | closed: no uploaded reports |
| RQ20 | Who assesses | only a diffpilot Codex job (read-only), on entering Comments and on new comments |
| RQ21 | Table view | separate page, `check-pr-comments` §4 columns, per-row select (default = recommendation) and reason/target, one submit; gate rows need their own choice |
| RQ22 | Data retention | as RQ14 |
| RQ23 | Notification channel without an open tab | b, extended: browser notification plus per-user ntfy (first-class) and generic webhook in Settings; agent tokens may register an ntfy or webhook push target; bounded retries; failures visible in Settings (§5.5) |

## 12. Assumptions to confirm
**[A]** review-body split ordinals stay stable across passes; **[A]** ≤ 10 items per job fits the prompt; **[A]** the
merge question repeats after each round from round 3; **[A]** `lastEditedAt` changes on thepastaclaw's in-place edits;
**[A]** Q-L reads cost about one point per page; **[A]** Watch constants (20 PRs, 5-minute probe, 90-minute bot-round
wait, 10-minute spacing) to be checked against S13-2 and the owner's usage; **[A]** notification constants (3 attempts,
10 s timeout, 30 messages per hour per channel, 30-day delivery log).
