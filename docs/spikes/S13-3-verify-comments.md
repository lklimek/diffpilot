# S13-3: `verify_comments` on recorded PRs: agreement with the owner, tokens, runtime

Date 2026-11-23. Question (plan §9.1): how well does the `verify_comments` prompt, at the installation effort, agree
with the owner's recorded decisions, and what does a pass cost? It calibrates `MAX_ITEMS` (T1), `PASS_SPACING` (T2),
`DAILY_PASSES_PER_USER` (T3), the job's effort (T4) and the prompt (T5). 10 real Codex runs (owner approval
2026-11-23). Raw runs (prompts, outputs, JSONL streams, sanitised): `/data/artifacts/diffpilot/2026-11-23/s13-3/`.

## 1. Setup

| ID | Item | Value |
|---|---|---|
| S1 | Runtime | `scripts/verify-comments-smoke.sh` (new): S1a image `diffpilot-spike-codex:0.167.0` (runner's Codex 0.167.0), login volume `sr-spike-codex-home` with the compare-and-swap credential copy of `schema-smoke.sh`; the runner's job layout (cwd = job dir, read-only checkout at `./repo`, `HOME` with the job kind's skills `severity` and `check-pr-comments` from claudius 13.2.1), the runner's per-job config and `codex exec --json --skip-git-repo-check --ignore-rules --output-schema`. Model `gpt-6.1-sol`. |
| S2 | Input | Rendered as the server renders it: template `prompts/verify_comments.md`, the shared rules and the data block from a Python mirror of `assess::request`, `prompt::render` and `markers` (normalise, title, heading, body findings); the mirror reproduces `prompts/golden/verify_comments.txt` byte for byte. No PR context, no project profile (none at the base), no earlier decisions (a first pass). |
| S3 | Replay point | The head and base the owner decided at (fleet log 2026-11-20, base `963cfa3987`), the PR description as edited last before the decisions (`userContentEdits`), thread lines at that head (unchanged on all 15 threads). Read-only `gh api graphql`. |
| S4 | Not reproduced | No `github-mcp-server` (no installation token is minted outside the stack), so rule 9 ("no GitHub tools, say nothing about related work") applied: `elsewhere` stayed empty in every run. The agent shared its uid with `CODEX_HOME` (S1a harness; the runner isolates it, plan §5.4). In these runs `CODEX_HOME` was under the agent's working directory `/job`, which the agent listed (file names only) while looking for dependency sources (R1, R2, R4, R5, R7, R9, R10); the script now keeps it in a separate `/codex` and checks the job output for credential values before archiving. |
| S5 | Validation | Every output against `schemas/agent/verify_comments.schema.json` (`schema-smoke-report.py verdict`), and every entry through the server's own `ingest::assessment` checks (a throw-away test, not kept). |
| S6 | Extra data | The live local stack's own passes of 2026-11-23 at the installation effort, from `docker logs` only (`job token usage`, `job phase timings`, pass ingest lines; stack PR id 28, repository not read). |

## 2. Pull requests and the owner's decisions

dashpay/dash-evo-tool, items as the server builds them (first pass: the first 10 assessable items in item order; for
#1158 and #1112 the open threads and the latest thepastaclaw round's body findings, see O3).

| ID | PR, head | Items |
|---|---|---|
| P1 | #1102 DashPay contact details through platform-wallet, `171e479670` | 10 threads (4 thepastaclaw, 6 coderabbitai) |
| P2 | #1158 panic messages kept out of DET-owned sinks, `9e4a56e06d` | 3 threads (2 coderabbitai, 1 thepastaclaw), 2 thepastaclaw body findings |
| P3 | #1112 one From/To Send form, `ff559d8ac3` | 2 thepastaclaw threads, 1 body finding |

How each decision was established (sources treated as data): the decision line of the coordinator fleet log
`/data/artifacts/dash-evo-tool/2026-11-20/fleet-tracking.md` (12:57–13:14, "USER …" lines, IDs of the triage files
`triage-<PR>.md` in the same directory), cross-checked with the PR's outcome on GitHub: the fix commit on the PR
branch, the "Per maintainer decision" reply, the thread state. No item relied on the log alone.

## 3. Agreement per item

`=` same decision kind as the owner, `x` different. H = high (installation effort), M = medium, H3 = high with the
first v3 wording of T5, H3b = high with its final wording (P1: R9; P2: R10, which adds T5b). Triage = the
coordinator's triage agent of 2026-11-20 (Opus, same skills), for comparison.

| ID | Item | Owner (evidence) | H | M | H3 | H3b | Triage |
|---|---|---|---|---|---|---|---|
| A1 | P1 C1 writer accepts only secp256k1 keys | upstream, platform#5546 (reply r4642967944, thread left open) | x fix_here | x fix_here | = | = | = |
| A2 | P1 C2 failed sent-request record only logged | follow-up PR #1195 (log; reply 4643295105) | = | = | x fix_here | x fix_here | x reject |
| A3 | P1 C3 acceptance published after failed record | follow-up PR #1195 (log; reply 4643295408) | = | = | x fix_here | x fix_here | x reject |
| A4 | P1 C4 document the key limitation | reject (reply 4642969066, resolved) | = | x fix_here | x fix_here | = | = |
| A5 | P1 C5 no expected-revision check | upstream, platform#5545 (reply 4642968696) | = | = | = | = | = |
| A6 | P1 C6 tolerant decoder reads rejected v0 | fix here (`aad9fcba`) | = | = | = | = | = |
| A7 | P1 C7 fee gate counts visible contacts only | fix here (`e1525efb`) | = | = | = | = | = |
| A8 | P1 C8 same as C1 (coderabbitai) | upstream, platform#5546 (decision of the merged row) | x fix_here | x fix_here | = | = | = |
| A9 | P1 C9 edits clear `acceptedAccounts` | upstream, platform#5657 (reply 4642968347) | x fix_here | x fix_here | = | = | = |
| A10 | P1 C10 same as C6 (thepastaclaw) | fix here (`aad9fcba`) | = | = | = | = | = |
| A11 | P2 C1 dash-async revision mismatch | fix here (`67e27cf4`) | = | = | = | = | = |
| A12 | P2 C2 backtrace follows `RUST_LIB_BACKTRACE` | fix here (`1f16929e`) | = | = | = | = | = |
| A13 | P2 C3 same as C1 (thepastaclaw) | fix here (`67e27cf4`) | = | = | = | = | = |
| A14 | P2 C4 raw join errors logged | fix here (`0a2f1ad1`, "fix if DET logs") | = | = | = | = | = |
| A15 | P2 C5 SPV panic text in connection details | reject, "owned by rust-dashcore" (log 12:57) | x fix_here | x fix_here | x fix_here | x fix_here | x fix |
| A16 | P3 C1 recipient rules in `ui/`, not `model/` | fix here, against the triage (log 13:12; `13fa0adf`) | x follow_up | x follow_up | — | — | x follow-up |
| A17 | P3 C2 test passes on any other error | fix here (`4a465392`) | = | = | — | — | = |
| A18 | P3 C3 Core fee missing from the total | fix here (`0d7c3242`) | = | = | — | — | = |

| ID | Effort, prompt | P1 | P2 | P3 | All |
|---|---|---|---|---|---|
| G1 | high, prompt v2 | 7/10 | 4/5 | 2/3 | 13/18 |
| G2 | medium, prompt v2 | 6/10 | 4/5 | 2/3 | 12/18 |
| G3 | high, first v3 wording | 7/10 | 4/5 | — | — |
| G4 | high, final v3 wording | 8/10 | 4/5 | — | — |
| G5 | triage agent (baseline) | 8/10 | 4/5 | 2/3 | 14/18 |

Every disagreement crosses the fix-or-not line (a code change in this PR or not); no kind was confused within
"handled elsewhere".

Disagreement causes:

| ID | Cause | Items | Prompt defect? |
|---|---|---|---|
| D1 | A fix that needs an unreleased dependency change (pin bump after an upstream PR) was `fix_here`; the owner records it as upstream and keeps the PR waiting | A1, A8, A9 | yes, fixed in v3 (§5) |
| D2 | The owner overrode the necessity gate (project rule "validation placement", fixed here); Codex and the triage followed the gate | A16 | no: the skill says only the owner overrides it |
| D3 | The owner rejected at decision time, then fixed the same sink at 15:31 as a new finding (C1158-07 `7eb8f1fb`, ManagerError `7bcdecfa`) | A15 | no: Codex matches the later decision |
| D4 | Borderline necessity of a documentation item flips between runs (H reject, M fix, H3 fix, H3b reject) | A4 | no: run noise |
| D5 | Pre-existing failures the PR now depends on: `follow_up_pr` in both v2 runs, `fix_here` in both v3 runs; the v3 reasons name a link the v2 runs missed (the PR's new detail saves need the established-contact state these failures leave unset). The owner fixed them in a follow-up stacked on #1102 (#1195), a scope-size choice | A2, A3 | open: possibly an effect of the v3 sentence, 2 runs each (O1) |

Facts beside the decision (high vs medium): pre-existing on base — high right on all checked items; medium marked
the tolerant decoder (A6, A10) as introduced by the PR, while the base has the same fallback
(`963cfa3987:src/backend_task/dashpay/contact_info.rs:249-261`). Gates: both efforts flagged G-DATA on A6/A10, which
the triage did not; high flagged G-FUNDS on A18 (wrong total shown, no loss). Every item had evidence (3–8 lines) and
a code range. coderabbitai's "Prompt for AI Agents" blocks were reported as data, as rule 6 asks.

## 4. Tokens and runtime

Input is cumulative over the agent's model calls (cached share in brackets); wall time includes container start.

| ID | Run | Items | Wall | Input (cached) | Output (reasoning) | Output size |
|---|---|---|---|---|---|---|
| R1 | P1 high | 10 | 300 s | 778k (667k) | 11.6k (3.9k) | 22.7 KB |
| R2 | P2 high | 5 | 142 s | 345k (271k) | 6.1k (1.5k) | 10.6 KB |
| R3 | P3 high | 3 | 129 s | 406k (326k) | 5.2k (1.4k) | 7.6 KB |
| R4 | P1 medium | 10 | 166 s | 417k (341k) | 6.8k (0.8k) | 20.8 KB |
| R5 | P2 medium | 5 | 106 s | 241k (186k) | 4.4k (0.4k) | 10.8 KB |
| R6 | P3 medium | 3 | 88 s | 239k (179k) | 3.5k (0.4k) | 7.3 KB |
| R7 | P1 high, first v3 | 10 | 297 s | 960k (856k) | 12.3k (4.3k) | 21.8 KB |
| R8 | P2 high, first v3 | 5 | 126 s | 327k (259k) | 5.5k (1.1k) | 10.7 KB |
| R9 | P1 high, final v3 | 10 | 301 s | 944k (839k) | 12.7k (4.5k) | 23.8 KB |
| R10 | P2 high, v3 with T5b | 5 | 146 s | 395k (326k) | 6.0k (1.3k) | 11.0 KB |
| L1 | live stack, high | 10 | 281 s | 1,858k (1,718k) | 11.0k | — |
| L2 | live stack, high | 10 | 276 s | 1,425k (1,286k) | 10.9k | — |
| L3 | live stack, high | 10 | 278 s | 861k (755k) | 11.4k | — |
| L4 | live stack, high | 3 | 122 s | 487k (391k) | 4.5k | — |

- Every output was schema-valid and complete (one entry per item). 65 of 66 entries passed `ingest::assessment`; R5
  C3 failed the plain-text check on the evidence note "The helper accepts ShutdownReport<WalletWorker>." (a Rust
  generic outside backticks reads as an HTML tag), so that item would stay "Not assessed" (fixed by T5b). The live
  passes L1–L4 (one 33-item detection chained as 10 + 10 + 10 + 3 in 16 minutes) were ingested `complete=true`.
- A pass costs a fixed ~250k input and ~1 minute (reading both skills, orienting in the PR) plus ~50k input and
  ~25 s per item at high. Medium halves input and runtime (R4 vs R1: −46 % input, −45 % time).
- Reference: a full analysis of P-CAP cost 1.58M input and 48k output (S7): one full pass ≈ half an analysis.
- The agent read both skills first in every run (`sed -n` on `SKILL.md`), then `git show <base>:…` for
  pre-existing checks; 12–57 commands per pass, no compaction, no usage-limit event, credential unchanged. In R7 it
  also listed Codex's MCP resources (none), looking for GitHub tools after the v3 sentence.

## 5. Decisions

| ID | Target | Decision | Rationale |
|---|---|---|---|
| T1 | `MAX_ITEMS` | **10** (unchanged) | A full pass takes ~5 min at high (R1, R7, L1–L3), 9× under the 45-minute job cap, 23 KB output. Larger batches save one fixed overhead (~1 min) per pass but delay the first questions; a 33-item read already chains in 16 min. In the spike, full passes cost less per item than small ones (O8). |
| T2 | `PASS_SPACING` | **10 min** (unchanged) | About twice a full pass, so one watched PR's automatic passes hold the single runner at most a third of the time; thepastaclaw rounds on one PR come about 30 minutes apart at the least (its push debounce), so the spacing rarely delays a round's pass. |
| T3 | `DAILY_PASSES_PER_USER` | **80** (was 30) | The owner's busiest recorded day (2026-11-21) had 44 thepastaclaw rounds (working model M8) and 156 assessed comments (comparison report D2). At 10 items per pass a round of n new items takes ⌈n/10⌉ passes, so that day needed about 44 + ⌊(156 − 44)/10⌋ = 55 automatic passes (every round at least one item, every extra pass 10 more; modelled on the sessions' 156 assessed comments; re-sends of undecided items whose lines changed and old-round body items (O3) are not included). Continuations and on-demand passes (questions, "Assess again", a page opened first) count against the same cap; on-demand is unmeasured, assumed 10: 65 passes, so 80 leaves 15 (about 23 %). Typical pass (3.5 items per round on that day) ≈ 0.4–0.5M input, ≈ 32–40M for 80 passes; worst case 80 full passes × 0.78–1.86M ≈ 62–149M input (86–92 % cached), below the 237M the session loop used that day. The prompt-version re-assessment (O7) also counts; see O10. |
| T4 | effort | **installation choice** (unchanged, high by default) | Medium cut cost by about half but lost one decision (A4), dropped the G-FUNDS flag on A18 and misread pre-existing code on 2 of 10 items (A6, A10), which feeds the necessity verdict. A medium default with a high re-assessment of funds, security, data-loss, privacy or gate items (comparison report R4) is a design change and was not made. |
| T5 | prompt | **v3**: one sentence in `recommendation`: a fix that first needs a change not yet made in a dependency or another repository is `upstream_issue`, not `fix_here`, even when this PR then only adopts it; `target_url` = the issue or PR the comment, description or code names | Fixes D1: A1, A8, A9 became `upstream_issue` with the right links (platform#5546, #5657, #5545) even without GitHub tools (R7). The first wording also said "`fix_here` only when this pull request can make the fix now"; R7 also turned A2–A4 into `fix_here`, so the final wording states only the exclusion. R9: P1 8/10 (v2: 7/10); A4 back to `reject`, A2/A3 still `fix_here` (D5). `VERIFY_PROMPT_VERSION` 3. |
| T5b | prompt | **v3**: the plain-text rule allows inline code and asks for backticks around code with angle brackets, as `prompts/parts.md` does for the other jobs | The v2 rule ("no Markdown") kept generics bare, which `analysis::verify::plain` rejects (R5 C3). R10: no regression on P2 (4/5, all entries fit), but no generic came up, so the case itself is unconfirmed. |
| T5c | prompt | **v3**: `resolve` only when the problem is gone at the head, naming in `reason` the short SHA of the commit whose change to those lines removed it (`git log -L`/`-S` from the head; the one that removes the problem when several touched them; "unsure" when unsure) and citing the head line in `evidence`; a `review_body` item's lines are at the review's commit, so the code is found by symbol | From the coordinator's investigation of the live stack (dash-evo-tool#1117, 2026-11-23, `/data/artifacts/diffpilot/2026-11-23/already-fixed/report.md`): all 19 `resolve` recommendations were right, none named the fixing commit; body-item lines were at old review commits in all 19. Not run on Codex here (10-run budget reached): O7. |
| T5d | prompt | **v3**: bounded reads (about 80 lines on each side of the discussed lines, room for the enclosing function the `code` field needs; no whole file over about 400 lines; the shared rule 8 of the other jobs is not among the rules this job renders); `elsewhere` also lists a `target_pr` or `target_url` that handles the matter, and without GitHub tools only links the comment, description or code names (the exception to shared rule 9) | Coordinator comparison `/data/artifacts/diffpilot/2026-11-23/pr-assessment-comparison/report.md` R10, R11: 40 % of command output bytes came from whole-file reads (W8); `elsewhere` was empty in all 61 assessments, including those with a target. Untested: O7. |

## 6. Open points

| ID | Point |
|---|---|
| O1 | One run per setting: borderline necessity items (D4, D5) flip between runs. Agreement figures carry about ±1 item of run noise; a larger sample needs owner approval for more runs. |
| O2 | `elsewhere` and GitHub look-ups were not exercised (S4); production passes have `github-mcp-server` with the PR-scoped installation token (ADR-005), whose reach to upstream repositories (dashpay/platform) is unmeasured. |
| O3 | The server makes items of the "not shown inline" findings of **every** non-dismissed thepastaclaw round review, not only the latest (`observe.rs`); old rounds' findings the owner settled long ago enter as undecided items and passes. This replay used the latest round only. Confirmed on the live stack: #1117 got 31 body items from 23 rounds, 19 of them fixed long before the head (coordinator investigation, T5c). Needs a decision (server change, not in this spike). |
| O4 | The title of a coderabbitai thread is its first line, `<!-- coderabbit-ai-generated:v1 -->` (`markers::title_of`): the queue shows an HTML comment as the item title. |
| O5 | Gate over-flagging (G-DATA on A6/A10, G-FUNDS on A18) makes gate items, which are never grouped and start empty on the table page. Not changed: the prompt already requires evidence per gate. |
| O6 | Cap and spacing rest on the owner's peak day and token counts, not on the plan's usage window (Codex `exec --json` reports no rate-limit percentages). Revisit after a week of live watch data. |
| O7 | T5c and T5d are not yet measured on real Codex; one high run on P1 with the committed prompt would check them (needs owner approval beyond the 10 runs). `VERIFY_PROMPT_VERSION` 3 makes every stored assessment stale, so the first read after the upgrade re-assesses every open item once (the live #1117 chain: 33 items, 4 passes, ~4.6M input). |
| O8 | Small tail passes cost more per item in the spike (input per item at high: 78–96k for 10 items, 135k for 3; medium: 42k and 80k; comparison report R5, W10); the live passes vary as much between passes (10 items: 86–186k per item, L1–L3; 3 items: 162k, L4). The server sends a tail at once: `next` requests the rest of a full, complete pass as a continuation, which skips the spacing. Holding a tail of 1–3 items for the next window needs new logic (a minimum batch for continuations, not for an owner waiting on the page). Not changed. |
| O9 | Necessity is the unstable field (D4, D5). The comparison report's R9 (quote the description sentence; pre-existing without a gate defaults to `out`) would move A2, A3 and A15 toward the owner but A6, A10 (pre-existing, fixed by the owner) only stay `in` through their G-DATA flag. A hypothesis; not changed without runs. |
| O10 | The job input carries no commit or date for an item (`VerifyItem`), so `resolve` cannot be bounded to commits after the comment and a body item's lines cannot be mapped from its round's commit; T5c asks for the commit whose change removes the problem instead. A later package adds the item's round commit to the input (change request); the rule can then use `<commit>..HEAD`. Continuations, on-demand passes and the re-assessment after a prompt-version bump all count against `DAILY_PASSES_PER_USER`; whether a version-bump re-assessment should be charged to a user is a design question (not changed). |

## 7. Input v1.3 and prompt v4 (2026-11-23)

Owner decisions on the comparison report (R2, R3, R8, R12, R13) and O10, ADR-023 amendment of 2026-11-23: bot bodies
without HTML, `<details>` and agent prompts; the description without CodeRabbit's summary and the PR Hygiene block;
per item the commit it was written on, the bot's thread status and the latest replies of its thread; reply-recorded
decisions as earlier decisions; prompt v4 with the gates, categories, necessity and checking rules in its own words and
no skills mounted. Both runs: high effort, `scripts/verify-comments-smoke.sh` (no skills), inputs built by the server's
own cleaning, context and render code (a throwaway test over the recorded reads,
`/data/artifacts/diffpilot/2026-11-23/s13-3/inputs/build-v4/`; the GitHub read itself is the recorded one, rounds
not derived, so an item's commit is its review's).

| ID | Run | Agreement | Wall | Input (cached) | Output (reasoning) | Requests | Tool calls |
|---|---|---|---|---|---|---|---|
| V0 | R9 above (P1, v3, skills) | 8/10 | 301 s | 944k (839k) | 12.7k (4.5k) | 37 | 50 (3 skill reads) |
| V1 | P1 replayed at `171e4796`, replies up to the decision time | 7/10 | 222 s | 756k (660k) | 9.4k (2.1k) | 32 | 48 |
| V2 | P1 at its later head `dc28c3eb` (base `0a3d516f`), every reply | 10/10 | 254 s | 769k (687k) | 11.0k (3.3k) | 44 | 49 |

Requests are estimated from the event stream as `Usage.requests` counts them (one per batch of tool calls and one
for the answer).

- V1 against the owner (A1-A10): A1, A5-A10 agree. A2 and A3 stay `fix_here` (D5, open: the necessity reason names
  the PR's new dependence on the established-contact state), A4 flipped to `fix_here` (D4: the documentation item
  flips between runs). Gates and pre-existing as in R9.
- V2 is the case the commit and replies are for: C6 and C10 `resolve` naming `aad9fcba`, C7 `resolve` naming
  `e1525efb` (the owner's fix) and the later `534a2523`; C1-C4 follow the decisions their replies record (C2, C3
  `handled_in_pr` #1195, C4 `reject`), C5, C8, C9 `upstream_issue` with the issues the replies name. The six resolved
  threads were sent as if open; the server sends only open ones.
- T5c holds: every `resolve` names a commit in `<commit>..HEAD` that changed the lines and cites the head line.
  T5d mostly: no whole file was printed; 5 (V1) and 7 (V2) of about 35 ranged reads exceeded 200 lines (R9: 11 of 34,
  longest 641; V1/V2: 456 and 451).
- `elsewhere` without GitHub tools holds what the comment, its replies, the description or the code names (V1: 5
  items, V2: 7).
- Size: the P1 data block went from 37.4 KB to 30.2 KB (bot bodies 17.7 to 7.6 KB, description 14.9 to 13.3 KB,
  2.9 KB of replies added); the template grew by 2.8 KB; the three skill reads (about 37 KB of tool output) are gone.
  Against R9, V1 used a fifth less input, a quarter less output and wall time.

O7 is measured (above). O10 is done: `VerifyItem.commit` (the bot round's head, else the review's commit). The
charging question of O10 is decided: a pass that only a new prompt version or the fingerprint format made stale
charges nobody (ADR-023 amendment 2026-11-23).

