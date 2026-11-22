# ADR-023: Review loop — maintainer decisions on PR review findings, acted on by external agents

- Status: proposed (2026-11-22; owner decision pending; open questions RQ1–RQ19 in `docs/review-workflow-plan.md`
  §11). Extends ADR-019 (personal access tokens: one write scope) and ADR-022 (plug-in activity rule).
- Plan: `docs/review-workflow-plan.md`. Mockup: `docs/mockups/10-review-loop.html`. Source of the workflow:
  `/data/artifacts/dash-evo-tool/2026-11-22/handoff/working-model.md` (cited "WM", with its IDs A, F, L, T, M).

## Context
The owner (WM A1) runs the PR review loop (WM L1–L11) through a coordinator Claude session (A2) in a terminal chat:
bots (A6, A7) and internal reviews (A4) raise findings, a verifier (A5) checks each against the current head, the
owner decides every comment fix per ID, one question at a time (WM §6.4), and the coordinator executes the decision
per a fixed matrix (WM §6.5). Measured: about 178 per-ID decisions a day, 40% of blocker entries are repeats, the bot
re-lists 76% of rejected items (WM M3–M5). Decisions live only in the tracking log (F9) and thread replies; no store
links an ID to its threads, recommendation, decision and executed actions (WM U1); IDs are not stable (U2).

diffpilot already reads GitHub threads with the user token, shows PR Hygiene statements (ADR-022), lets the user's
agents read with personal access tokens (ADR-019) and keeps its own agent read-only (T3, ADR-005).

## Decision
1. **Module.** A "Review loop" module: `server::review_loop` (ingest, queue, decision effects, log),
   `server::github::review_loop` (reads), `db::repo::review_loop` (one append-only migration), contracts
   `api::review_loop`, web `components/review-loop/**`, `screens/review-loop/**`. No new crate; no runner or
   `JobExecutor` change in phases 1 and 2.
2. **diffpilot is the decision console and ledger; the coordinator acts.** The coordinator uploads verifications and
   internal reviews, reads decisions, answers the owner's questions and reports executed actions. It keeps its own
   GitHub identity (`Claudius-Maginificent`, `ghsudo`) for commits, pushes, replies, resolves and PR-body edits.
   diffpilot performs no GitHub write in phases 1 and 2 (its pending-review comments are unchanged).
3. **Decisions are owner-only** (WM §10 item 1). Recording, superseding or reopening a decision, marking or clearing a
   gate, and "resolve this gate thread" need a browser session with CSRF. No token scope can decide. The other WM §10
   actions (merge, `/self-reviewed`, `/skip-bots`, dismissals, retargeting, workflow edits, force-push, `ghsudo`
   approvals) are never performed by diffpilot; it only lists them as waiting for the owner, with GitHub links.
4. **One write scope for agents.** ADR-019's extension point gets `review_loop:report`: a bearer token with it may
   POST verification and internal-review uploads, answers to the owner's questions, action reports and a re-read
   request. It needs no CSRF (no ambient credential; dashboard plan §E5-8). Every other mutating route keeps refusing
   tokens.
5. **Effects are computed by the server.** A pure function maps (decision, author kind, source kind, gate,
   override) to the expected effects of WM §6.5 (fix, reply text, resolve, PR-level comment, body edit, follow-up PR,
   upstream issue link, TODO, held local commit reset or move). The coordinator executes them and reports each one;
   it does not re-derive them. A gate thread (G-FUNDS, G-SECRET, G-CRYPTO, G-DATA; WM §6.5 special case 1) never gets
   a resolve effect unless the owner's decision says "resolve this thread".
6. **Stable keys and IDs.** A bot finding's key is its `dedupe=` marker (WM F3, U2); other threads use the thread
   node id; body items the source URL plus the verifier's ordinal; internal findings the report and finding id. Each
   item gets a per-PR number (`C1`, `C2`, …) at first sight, never reused; `CMT-…` and `QA-…` are aliases of one upload.
7. **Repeats.** A finding whose key matches a decided item is listed as "repeat of C7" and not asked again
   (WM §6.3, practice since 2026-11-21) unless it brings new evidence; the earlier decision's effects are carried
   (pending RQ3).
8. **Internal findings follow WM §6.6.** They are the coordinator's, shown read-only with its disposition; only the
   three exception classes (product-rule ambiguity, overlap with an open thread, pre-existing gate finding) enter the
   owner's queue.
9. **Upload formats.** Verification items follow the per-ID contract of WM §6.2 (claudius TODO T-3c2b5f03), defined
   in `crates/contracts` with a generated JSON Schema; internal reviews upload claudius review reports (schema 4.0.0
   or 4.1.0) as they are, wrapped with the PR, the reviewed range and reviewer metadata (WM Z12).
10. **The runner stays read-only.** If diffpilot later posts replies or resolves threads itself (phase 3, RQ2), the
    server executes them with a server-held credential after the owner's decision; no agent (Codex job or external)
    receives a GitHub write credential from diffpilot.
11. **triage-findings is superseded for PR-comment decisions** on tracked pull requests; it stays for local review
    reports without a pull request. Its `triage` overlay is not produced.
12. **Trust and wording.** Brief §1 applies: agent text carries the "LLM" tag and its reporter; GitHub text the
    "GitHub" tag; badges only for facts diffpilot read itself (thread resolved, outdated, edited, reply present,
    marker parsed); a bot's "Blocking" is shown as the bot's claim (WM §11); no numeric confidence (report floats and
    `ai_verdict_confidence` are never shown); no "verified" / "no risk" / "noise"; an empty queue states what was read.
13. **Plug-in activity.** A browser-session read of a review-loop page marks the reader active for ADR-022 polling,
    like the dashboard summary.

## Alternatives considered
| Alternative | Why not |
|---|---|
| diffpilot jobs fix code, push, reply and resolve | the runner is read-only with no code execution (brief §2 Agent access, T3); a prompt-injected PR would gain write access; WM keeps pushes with the coordinator (WM §8) |
| diffpilot holds the coordinator's or `ghsudo` credentials | third-party credentials leave their owner; `ghsudo` needs per-action owner approval (WM §10 item 10) |
| Server posts replies with the owner's App user token from phase 1 | changes who speaks on GitHub (today the coordinator's account, WM A2) without an owner decision (RQ2) |
| Keep triage-findings and add a cross-PR index | one local server per report, no memory across rounds (repeats re-asked), no PR status, its actions lack move, upstream issue, TODO and gate override |
| Upload claudius comment-check reports (schema 4.1) for verifications | lacks pre-existing, category, necessity verdict, fix size, two reply variants and local-fix fields of WM §6.2 |
| Agents read decisions from the tracking log | no access control, audit, gate enforcement or conflict detection; WM U1 |

## Consequences
- New tables, routes, contract types, an ADR-019 amendment (one write scope on four routes) and threat-model rows
  T22–T25.
- claudius `check-pr-comments` (interview mode T-9f89cc97, verify-only contract T-3c2b5f03) and the coordinator
  playbook need a diffpilot mode: upload, read decisions (SSE or polling), answer questions, report actions. That work
  is outside this repository (WM decision M-2 decides where it lives).
- The recommendation is stored next to every decision (WM R11(b)), so agreement and repeats become measurable.
