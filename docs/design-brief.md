# diffpilot — design brief

Status: Design approved 2026-11-16. Implementation follows `docs/implementation-plan.md`.

This file is the single source of truth for the design. Requirements, ADRs, architecture and mockups derive
from it. Approved mockups: `docs/mockups/` (open `index.html`). The mockups are illustrations with example
data; where a mockup and this brief differ on behaviour, the brief wins.

## 1. Product

**Problem.** A classic diff does not tell a reviewer what happened: moved code shows as removed + added,
production code and tests are interleaved, and nothing explains what the change does or whether it is the
right change for the project.

**Product.** A self-hosted web tool that helps a HUMAN review a GitHub PR. It shows the FINAL code with
changed regions highlighted, grouped into flows, and walks the reviewer through them one step at a time. Each
step says what changed in behaviour, why, the consequences, the call hierarchy, questions to check and a
system-scale risk assessment.

**Terminology.** A **flow** is a logical change following control flow across files. The data model calls it
a "group" (entity `Group`); the UI and this brief say "flow". A **step** is one fragment of a flow.

**Purpose beyond the diff: avoid the local optimum.** A PR can be correct in isolation and still be the wrong
solution for the project (duplicate mechanism, wrong layer, competing PR, oversized for the problem). The tool
gives the reviewer the PR context (problem, related work, fit questions, CI, project profile) so the reviewer
can judge the PR at project scale, not only line by line.

**Principles**
0. Keep it simple: the simplest design that meets the scope; no speculative abstraction. Prefer removing or
   rewording over adding UI.
1. Not an automated reviewer. The human decides; the tool directs attention.
2. Attention is the scarce resource: the screen shows only what is to be read now (one block at a time); the
   most important thing comes first; quiet material comes last and is never ranked above a rated flow.
3. Final code, not old-vs-new. The old version is available on demand per step (deleted code is the one
   exception: shown by default at the deletion marker).
4. Every changed line belongs to exactly one flow or to a Not analysed section (coverage invariant); lines the
   agent failed to analyse go to "Unexplained — read these yourself", listed first among the quiet rows.

**Trust rules** (a wrong "pure refactor" or "no risk" label hides bugs; wording is part of the design)
- **A badge names exactly what was checked.** Only deterministic facts get a badge: "text identical",
  "classified by path", "classified by line rule". Everything else is LLM analysis; one line per panel says
  "LLM analysis unless marked otherwise". The blanket word "verified" is not used. A green tick is only for
  deterministic content checks.
- **Silence is not safety.** Never "no risk", "nothing to review", "no review needed", "noise". Material the
  tool does not analyse is "Not analysed". When the LLM raises nothing: "The LLM raised no questions. That is
  not evidence of no defects."
- **Verdict after evidence.** Evidence (behaviour, why, consequences, call hierarchy) comes before
  assessment; assessment is collapsed until clicked. LLM suspicions are phrased as questions to check, not
  verdicts. The remark marker's popover (opens on hover or focus of the marker; a click pins it) states the was → is fact of a behaviour remark: no risk, no verdict; a highlight has no hover tooltip.
- **No numeric confidence** anywhere (no decimals, no percentages). Observed facts carry a line reference;
  inferences are marked as inferences; hedges survive shortening.
  Review ticks must not become points or a percentage score.
  The analysis waiting screen may show stage counts, overall progress as a percentage and a clearly labelled ETA;
  these describe processing work, never confidence or review quality.
- **LLM-classified refactors** (LLM says "pure refactor" or "rename", no deterministic confirmation) are
  shown as normal flows with that label, never collapsed (§3, §4.3).

### Addendum 2026-11-18: risk statements in Ask answers (owner decision)
- For Ask answers a risk is stated, not asked: a short title naming the risk class prefixed "Potential …" (for
  example "Potential TOCTOU race:"), a numbered scenario (who acts, the order of events, the outcome and its cost)
  and one closing line saying what was not reproduced or checked. Owner: concrete risk scenarios are better than
  questions and prose.
- It stays an LLM statement, shown as LLM analysis, never called verified, with no numeric confidence. The absence
  of such statements is never shown as safety.
- Flow analysis keeps "questions to check" for now; the same form replaces them in a later iteration.

### Addendum 2026-11-18: flags replace questions in flow analysis (owner decision)
- A suspected defect in a step is a **flag**: "Red flag" (the LLM sees a likely defect or damage if the scenario is
  reachable) or "Yellow flag" (worth a look: plausible but weaker, or of lower cost). It has a title naming the risk
  class ("Potential …"), a scenario of 2 to 7 ordered steps (who acts, the order of events, the outcome and its cost)
  and one line of what was not reproduced or run. A risk the LLM cannot state that way is not a flag. Owner: "call it
  simply yellow flag / red flag and describe what it is about"; concrete scenarios are easier to understand than
  questions and prose.
- A flag is an LLM statement: labelled as LLM content, never called verified, no numeric confidence; its colour is
  never the only signal (the level is also written). The absence of flags is never shown as safety.
- Inspector: the flags of a step replace "Questions to check" and are visible without opening the collapsed
  Assessment. Each is a card: level, title, the scenario as a numbered list showing at most 5 lines at once with
  "Show all" / "Show less", the not-checked line and its evidence. The code view draws a flag's bar in its level's
  colour; the step rating stays per step, but a step with a red flag is not rated LOW. Navigation pills keep the
  rating; their tooltips count flags by level.
- Analyses stored before flags are not converted or analysed again: their questions show as flags of unknown level
  ("Flag"), with the question as the title and no scenario. Supersedes "questions to check" for flow analysis in §1
  (Product, Verdict after evidence), §3 (Annotation) and §4.3 (Inspector, section 6; hard case 3 becomes a step on
  which the LLM raises no flags).

## 2. Scope

### v1
| Area | Decision |
|---|---|
| Screens | Four: Home, Analysis progress, Tour, Settings. The Tour is the whole review screen. |
| Input | One field: GitHub PR URL. Public repositories only. No dashboard, no recent-reviews list. |
| Users | GitHub login (GitHub App, user access tokens). The GitHub logins allowed to sign in are a tenant setting; v1 has no UI or configuration file for it: the seeded tenant `default` allows exactly `lklimek`. Single tenant `default`, hardcoded. |
| Analysis | The review opens only when the analysis has finished. No partial review. |
| PR context | Problem and why, related work, fit questions, CI, deterministic counters, project profile status. |
| GitHub context | Existing review threads at their steps, with an LLM status; CI status. |
| Ask | Free-text question about the current step, answered by an on-demand agent run with repo access that resumes the analysis's agent session, pinned and labelled as an LLM answer. In the MVP. No one-click deepen actions. |
| Progress | Tick off steps; a flow is reviewed when all its steps are ticked; top-bar counter "flows reviewed k / n"; persisted per analysis, together with the last viewed block. |
| Comments | On any displayed line; written straight into the user's GitHub pending review (a line outside the PR diff becomes a file-level comment that links to the line). No local drafts. |
| Feedback | correct / wrong / irrelevant / overrated / underrated + note; "missed by the LLM" recorded automatically. Stored locally and written to MemCan. |
| Resume | Opening a PR with an existing analysis resumes it (ticks, answers, last viewed block kept). If the head moved: banner and "Re-analyze" (fresh analysis, ticks not carried over). |
| Project profile | Union of `.claudius/project-profile.md` on the base ref and MemCan `type: project-profile`; the repository file wins on conflict. Neither: analysis runs without it. |
| Related repositories | Auto-detected from `Cargo.toml` / `Cargo.lock` (git and path dependencies), same GitHub organisation only. No manual configuration. |
| Language | UI English. LLM texts in the user's configured language (default English). |
| Languages of code | Any, LLM-only. Built-in `generic` tech plugin (path globs and line regexes). |
| PR size | No limit. Huge PRs (10k+ lines) are split into many agent runs; long runtime is acceptable. |
| LLM | Codex CLI on a ChatGPT Pro subscription (no 5-hour window). Installation-wide model and reasoning effort in Settings, default Sol/high (§5 Model selection). Provider trait designed for a second provider. Usage-limit pause and resume must work but are not a first-stage priority. |
| Agent access | Read-only checkout, no code execution. MemCan MCP (read and write), the `claudius` plugin, GitHub's `github-mcp-server` in read-only mode with a short-lived read-only installation token, related repos as extra context. |
| Deployment | docker compose, Portainer-compatible. Public base URL is configuration (`SR_PUBLIC_URL`; the owner's instance: `https://review.klimek.ws`). TLS is terminated by an existing reverse proxy; the server speaks HTTP behind it. |

### Later
- Later versions (not yet assigned to a number): Groups workspace (flows as a navigable list with a
  per-flow code view), overview screen, files tree, partial review while the analysis is running.
- v2: re-review (semantic delta since the reviewed head, ticks carried over); recent-reviews list; LLM
  "auto-comment" (drafts an English PR comment); Claude provider; Rust and Go tech plugins (tree-sitter
  test/comment ranges, LSP call hierarchy, deterministic rename / syntax-level confirmation of refactors);
  PR-description-vs-code check.
- v3+: LLM-curated dashboard of PRs to review; per-job containers (tenant tokens, private repositories,
  later build/test).
- Later, unscheduled: static-analysis actions (find callers, test coverage, verify suspicion); sync of ticks
  with GitHub "Viewed"; custom "not analysed" rules.
- v5+: real tenants (tenant = organisation, own LLM keys, users ↔ tenants many-to-many), billing.

### Non-goals
Automatic approve/reject. Publishing reviews without the user. Replacing CI. Local comment drafts. PR-level
overall risk score or size statistics. Numeric confidence. A setting to show or hide verdicts.

## 3. Domain model

```
Tenant 1─* Repo 1─* PullRequest 1─* Analysis (one per head SHA + language)
Analysis 1─1 PrContext
Analysis 1─* Group (= flow) 1─1 FlowGraph 1─* Fragment (= flow step) 1─* Annotation
Group    1─* PatternRepetition
Analysis 1─* NotAnalysedSection 1─* Item (path- or line-rule matched; moves with identical text)
Fragment 1─* ImpactRef (code outside the diff);  Fragment 1─* Question (Ask thread, LLM answers)
User *─* Tenant;  User 1─* ReviewState (per analysis): ticks on fragments, last viewed block
User 1─* Feedback (→ MemCan);  Feedback may reference a Fragment, an Annotation or a plain changed line
```

Comments are not stored locally; they exist only in the user's GitHub pending review. A flow is reviewed
when all its fragments are ticked (derived, not stored).

**PrContext** — the problem the PR solves and why it is needed, and what the change gives; related work
(each item: link, kind — duplicate, competing solution, stacked PR, linked issue, companion repository — and
one line on the overlap); fit questions with evidence (right layer / repository? oversized for the problem?
duplicates an existing mechanism?), phrased as questions, not verdicts; CI status (failing check and test
names); counters from the deterministic pre-pass ("N lines removed from tests", "M dependencies changed");
project profile status (found with source, or "no project profile — ratings are component-level").

**Flow (Group)** — a logical change following control flow (trigger → calls → state changes → result) across
files, including unchanged code that connects the changes. Fields: title, classification, importance
(ordering), flow introduction, flow graph, step order, dependencies on other flows.

**Flow introduction** — three fields:
| Field | Content |
|---|---|
| Problem (before this PR) | Local problem: trigger, what was wrong or missing |
| Change (what this PR does) | The intended correction |
| Before / After example | A short concrete example of behaviour before and after |

The label "Was wrong" is not used.

Classification (closed list): `behavioural`, `api_contract`, `new_code`, `refactor_pure`, `move`, `rename`,
`test`, `docs_comments`, `generated_mechanical`, `config_build`, `dependency`, `other`.
`test`, `docs_comments` and `generated_mechanical` material recognised deterministically (path or line rule)
forms the Not analysed sections. Test code that only the LLM recognises (for example a `#[cfg(test)]` module
inside a production file) is shown as its own flow at the end, labelled "LLM-classified tests"; it is never
hidden, because only deterministic checks may hide material. `refactor_pure` and `rename` are LLM claims: a
flow so classified is an ordinary flow labelled "LLM-classified refactor".
The only quiet rows for moves are those whose text is deterministically identical and that leave a still existing file for another one, or pure renames (amendment 2026-11-18).

**FlowGraph** — structured data, not diagram syntax; the UI renders it. Nodes: id, step number, kind
`added` / `changed` / `unchanged` (unchanged = context), fragment reference, label. Edges: from, to, branch
label (decision outcomes, loop back-edges, error exits). Each node is the diagram twin of one fragment.

**PatternRepetition** — a pattern applied in several places is explained once (in a step); the other places
are listed as a collapsed row "same pattern × N", each with a step reference.

**Fragment (step)** — one step of a flow: a line range of one file at the head commit, numbered like its
diagram node. Fields: file, symbol, GitHub line link (head SHA), called by, decides/changes, next (where
execution goes), why (why this step or modification is needed), kind, annotations, collapsed assessment.

| Kind | Meaning | Visual |
|---|---|---|
| `modified` | logic changed in place | highlight on annotated ranges |
| `added` | new code | "new" highlight |
| `moved` | block moved; badge "text identical" or "N lines differ" | muted block; only the differing lines highlighted; identical moves are quiet rows |
| `renamed` | identifier rename only | faint underline |
| `deleted_site` | code removed here | removed lines shown by default, removed capability named ("Removed the single-attempt API") |
| `context` | unchanged code connecting the flow | neutral, distinct border style |

Every changed line has a neutral gutter mark (deterministic fact). LLM colouring is laid on top, only on
annotated ranges, so "changed, no LLM remark" is its own visible category.

**Assessment** (per step, collapsed by default): headline = what a production user or operator would notice;
risk level at system scale with reason (also for LOW); basis (what the agent did: "Read: 2 files", "Ran: …" if
it ran anything) and what was not checked; introduced by this PR or pre-existing; blocks this PR or follow-up; smallest correction. The
risk shown for a flow in the menu is LLM-rated; there is no PR-level risk.

**Annotation** — anchored to lines inside a step. Types: `behaviour` (was → is), `consequence`, `question`
(questions to check, including LLM suspicions of bugs), `assessment`, `note`. Each carries evidence
references (file:line). Observed facts are marked as facts; inferences as inferences.

**Call hierarchy** — callers and callees per step symbol, each with a GitHub link. It comes from the tech
plugin (`call_hierarchy`), never from the LLM. In v1 the plugin returns nothing, the field is empty and the
UI shows "n/a". **Provenance badges** (deterministic only): `text identical` (moved block equal
after whitespace normalisation), `classified by path`, `classified by line rule`; `git color-moved` is
internal evidence for move candidates. No badge means LLM analysis.

**Feedback** — values: `correct`, `wrong`, `irrelevant`, `overrated`, `underrated`, `missed by the LLM`,
plus optional note. `missed by the LLM` is created when the reviewer comments on a changed line that has no
annotation. `overrated` / `underrated` also become calibration examples in MemCan.

**Candidate JSON** emitted by the agent: base shape in `prior-art.md` §5 (Background material); full schema
produced at requirements time (`schemars`). Fields: per flow — title, classification, importance,
flow introduction, flow graph, steps, pattern repetitions, dependencies; per step — file, range, symbol,
kind, called by, decides, next, why, annotations with evidence references, impact refs,
assessment; per PR (PR-context job) — problem, related work, fit questions. No confidence number, no overall
PR risk.

## 4. Screens

Rules for every screen:
1. Use the full page width (the owner works on a 21:9 monitor); no narrow centred column. Light and dark
   themes follow the system setting, as in the mockups.
2. The PR number in the top bar opens the PR on GitHub in a new tab.
3. Every label (risk level, classification, provenance badge, status, button, column header) has a tooltip
   explaining it, except the code-view highlight label, whose explanation is the remark popover.
4. The legend (how code is drawn, §4.3) is not on the page; it is in the `?` help together with the
   keyboard shortcuts.
5. Review-screen top bar: repo, PR number and title, head SHA, CI status (failing checks in the tooltip),
   counter "flows reviewed k / n" (no percentage, no risk weighting), pending-review count, help, user menu.

States every screen needs: loading, empty, error.

### 4.1 Home
One field (PR URL) and "Analyze"; signed-out state "Sign in with GitHub"; error states for a URL that is not
a PR, a repository without the GitHub App installed, and a GitHub login the tenant does not allow. After a
cancelled analysis Home shows "Analysis cancelled — analyze again". A PR with an existing analysis is
opened ("Open": ticks, answers and last viewed block kept, nothing re-analysed). If the head
moved: banner "This analysis is for `abc123`; the PR is now at `def456`" with "Re-analyze" (fresh analysis,
ticks not carried over) and a link to open the old analysis.

### 4.2 Analysis progress
A centred overall analysis progress bar and Cancel analysis, with a compact list: Planning (parts done / total),
PR context, Flows (done / total; known after planning), Finishing. Multi-part planning names the current part;
its timer and the whole analysis timer are separate. Show a labelled estimate with its basis when available,
otherwise "No estimate yet"; a pause shows its resume time instead. No agent activity feed. States:

| State | Shows |
|---|---|
| Planning | overall bar when estimable, otherwise indeterminate; current plan part, Cancel analysis |
| Running | bar, "k of n flows analysed", Cancel analysis |
| Paused | Usage limit: resume time; user pause: who paused and when, Resume |
| Failed | which flow failed, or a plain description of what failed before any flow existed; Retry |
| Finished | "n of n flows analysed", Open |

The review opens only when the analysis has finished. Pause analysis lets the running job finish, keeps
its result and session, and submits no further job until Resume, including after restarts. A user pause
is independent of a provider limit and never resumes automatically. Resume checks that the PR head has
not moved. Home also offers Resume. Cancel has an inline confirmation explaining that it discards the
plan and completed flows while retaining both PR sessions; Pause keeps everything. Confirmed cancellation returns to Home.

Owner-approved amendment (2026-11-17): once planning and PR context are accepted, the Tour opens while
flows continue serially. Pending flows show their planned intent/ranges and explicit work state, never
an implied assessment. Opening a pending flow requests it ahead of background work; the current job is
not interrupted. The per-analysis background switch defaults on and survives restart; off means on demand.
Accepted flows support review state, comments and private Ask immediately. Pause, usage limits and Cancel
retain their existing semantics. This supersedes the completion barrier above because the measured large-PR
run produced about 184k output tokens in over 80 minutes before any review was readable.

Owner request (2026-11-19): with "Notify me when work finishes" on (Settings, per browser, off by default; the
browser's permission is asked when it is turned on), an open diffpilot tab that is hidden or unfocused shows a browser
notification when the user's Ask answer arrives or fails, a flow is analysed or fails, the review becomes ready, or
the analysis finishes or fails: PR and flow or step only, never answer text; LLM answers are labelled as such; one
per event across tabs; a click focuses the tab and opens the step or flow. No service worker: closed tabs, http
outside localhost and mobile browsers get none.

Owner-approved assessment amendment (2026-11-17): the lowest band needs only its band and one sentence
reason; higher bands retain the full assessment. Detailed fields are optional in the data model and omitted
for concise low assessments. Assessments remain collapsed by default; no step-count cap or per-kind effort.

### 4.3 Tour (the review screen)

**Sequence.** PR context → for each flow in order: flow introduction → its steps, one at a time → next flow.
Previous / Next walk this sequence; after the last step of a flow comes the next flow's introduction. The
first visit starts at PR context; reopening a PR returns to the last viewed block. Flow order: flows that
change behaviour by importance first; then LLM-classified refactors; then LLM-classified tests; then quiet rows.

**Layout.** Three columns — left menu | centre | inspector. Column widths are resizable by dragging and
remembered. Each column has "hide" and "full screen". A hidden column leaves a strip with its name;
clicking anywhere on the strip restores it.

**Left menu**, in order:
1. PR context.
2. Flows in the order above (title, risk, tick; LLM-classified refactors carry their label); the current
   flow lists its numbered steps.
3. Quiet rows, always last and never above a rated flow: moves with identical text; Not analysed sections
   (Tests, Docs & comments, Mechanical & generated) with deterministic counters; "Unexplained — read these
   yourself" lines. Each opens in the centre.
4. Pending review: count and link; the list opens in the centre.

**Centre**, split horizontally.
- Top: the flow diagram, hidden by default ("Show flow" toggle). Numbered steps, decisions with labelled
  branches, loops, error exits. Added, changed and unchanged-context nodes differ by colour AND a second
  signal (border style or marker). When shown, the current step is marked; clicking a node goes to that step.
- Bottom: ONE block, never two side by side, at any width. Under it: Previous / Next.
  - *PR context block*: contents as in §3, plus what the change gives and why it is needed.
  - *Flow introduction block*: title, flow risk, Problem (before this PR), Change (what this PR does),
    Before / After example, "LLM analysis unless marked otherwise".
  - *Step block*: header (number, title, kind, file, symbol, GitHub line link at head SHA, new tab); four
    facts always visible — called by, decides or changes, goes next, **Why**; the code; impact outside the
    diff inline under the step as real code; "Same pattern × N" collapsed rows; collapsed Assessment; "Show
    old code" (inline classic diff); "Mark reviewed" tick. Final code only, except deleted code (shown at the
    deletion marker with the removed capability named).
  - *Menu-row blocks* (quiet rows, Not analysed, Unexplained, pending review): the list behind the row. Not
    analysed reads "Recognised by file path, no LLM: Tests (4 files), Docs & comments (3 files), Cargo.lock"
    plus the deterministic counters.
- A moved block with identical text reads "Body text identical; call site changed in flow 1".

**Code view.** Own line renderer. Gutter: neutral mark on every changed line; LLM colouring only on annotated
ranges; markers for LLM remark, deleted site, GitHub thread, own comment. A highlight has no hover tooltip; clicking its remark marker opens a popover with Was and Is as separate blocks.
Legend (shown in `?`):

| Drawing | Meaning |
|---|---|
| Neutral gutter bar | changed line (deterministic, from the diff) |
| Solid / dashed / dotted bar | HIGH / MEDIUM / LOW or affected (LLM rating, annotated ranges only) |
| No colour on a changed line | no LLM remark |
| Double bar, `≠` | moved, text identical; line differs from the moved original |
| Unmarked code | outside the diff, unchanged |
| Removed code at a marker | deleted site |
| Diamond / square marker | LLM remark / your comment |

**Inspector** (right). Never empty. It starts as a strip only when hidden by the user.
- No highlight selected: it describes the current block. For a step: classic diff of the step, behaviour
  (was → is), why, consequences, call hierarchy, GitHub threads, with Questions and Assessment collapsed. For
  a flow introduction or PR context: the matching summary.
- Click a highlight: the inspector narrows to that annotation; "Back to step" returns.
- Sections, in order, each collapsible (collapsed state remembered in localStorage):
  1. Classic diff of the selected range.
  2. Behaviour (was → is).
  3. Why.
  4. Consequences.
  5. Call hierarchy — callers and callees with links to file and line on GitHub (new tab). In v1 the
     section shows "n/a — needs a language plugin".
  6. Questions to check — LLM suspicions phrased as questions ("Can `broadcast_tx` be sent twice when the
     reply arrives late?"). If none: "The LLM raised no questions. That is not evidence of no defects."
  7. Assessment (collapsed by default): fields as in §3.
  8. GitHub threads at this location. Threads located in the flow's steps carry an LLM status from the flow
     analysis: still valid / stale / already addressed; other threads have no status.
- **Dock** (below the sections, works at step level without selecting a highlight):
  - Ask — one-line text field that grows while typing; answers stay pinned to the step, labelled as LLM
    answers.
  - Feedback — correct / wrong / irrelevant / overrated / underrated, optional note.
  - Comment — on ANY displayed line, annotated or not. While it is being written the line is highlighted;
    after posting the line keeps a comment marker. "Add to GitHub pending review" writes straight into the
    user's pending review on GitHub. GitHub accepts line threads only inside the diff, so a comment on a line
    outside the PR diff is added as a file-level comment whose body links to the line.

**Progress.** Tick per step; a flow is reviewed when all its steps are ticked; top bar "flows reviewed k / n".

**Keyboard**: `j` / `k` next / previous block (introduction or step), `n` / `p` next / previous flow, `x`
tick the current step, `d` show / hide old code, `c` comment on the current line, `a` ask, `?` help, `Esc`
close.

**Three hard cases the mockup shows as switchable states:** (1) a confident but wrong HIGH, marked wrong /
overrated; (2) a defect in a plain changed line without annotation, commented by the reviewer and recorded as
"missed by the LLM"; (3) a PR where the LLM raises no questions. Further mockup states: no project profile;
returning to an interrupted review.

### 4.4 Settings
- LLM model and reasoning effort: two selects from a fixed server list, one saved choice for the whole
  installation. Show who changed it and when; saving is read-only while pending. Changes apply to later
  submissions, including remaining jobs of running analyses, with Sol/high as the default.
- LLM status: the outcome of the most recent LLM call — "working" if it succeeded, otherwise "not working"
  with the plain error message — and the time of that call. No on-demand check. Credentials never reach the
  browser. The first Codex login is a one-time manual command in the runner container (runbook); there is no
  login UI.
- Language of LLM texts.
- GitHub App installation status.
- Per repository, read-only: built-in "not analysed" rules (path globs and line regexes), related
  repositories (auto-detected), project profile source. Use a searchable keyboard-operable repository picker,
  latest analysis first then name, with per-user browser memory and a `repo=owner/name` URL parameter.
  Cards take their content height; the built-in rules are collapsed by default. Format MemCan call times
  like LLM call times, separately from the project-profile line.

## 5. Architecture

```
browser ── HTTPS ──> reverse proxy (TLS) ── HTTP ──> server (axum: JSON API + SSE, embedded SPA, orchestrator, SQLite)
                        │  job queue (DB); JobSpec carries a read-only installation token   │ GitHub API (user token; App key)
                        ▼                                                                    ▼
                     runner (Rust + codex CLI + github-mcp-server, git, ripgrep) ── egress proxy (Squid) ──> LLM, MemCan, GitHub
                        own checkouts: per-job work dir + local mirror cache
```

- **server** — API, sessions, GitHub integration, own bare mirror (file content for the UI, pre-pass),
  deterministic pre-pass, related-repository detection, validation of agent output, persistence, SSE
  progress, pending-review writes, minting of a read-only GitHub App installation token per job. Holds the
  GitHub App private key and the users' tokens; user tokens never leave the server. Trusted. It speaks HTTP
  behind the owner's TLS reverse proxy (trusted-proxy setting, Secure cookies); its public base URL is
  configuration (`SR_PUBLIC_URL`). No built-in TLS or ACME.
- **runner** — a long-lived compose service, no Docker socket. Takes one job at a time per LLM credential
  (Codex rotates its token on refresh). For each job it fetches the repository itself (local mirror cache),
  checks out the head into a fresh work dir, runs `codex exec --json --output-schema` there, streams events
  back and deletes the work dir. `CODEX_HOME` on a volume, isolated from repo config. The container is the
  sandbox: non-root, on an internal network whose only egress is the proxy service. It has no database
  access; the agent holds only the job's short-lived read-only installation token, never a user token.
- **egress proxy** — Squid, used by the runner as an explicit proxy (environment variables). v1 policy: allow
  all public destinations, deny the private LAN except MemCan, and log every destination host (CONNECT
  host:port, bytes) to learn which hosts are needed. No TLS interception and no cache in v1. Squid is chosen
  because the same proxy can later, by configuration only, intercept TLS (full-URL logs, own CA in the runner
  image, `chatgpt.com` / `openai.com` passed through), cache downloads and enforce a host allowlist.
- **claudius plugin delivery** — the runner keeps a git checkout of `lklimek/claudius` on a volume. Before a
  job, if the last refresh is older than a configured interval, it runs `git pull --ff-only`. It exposes the
  checkout's skills directory to Codex. The ref is configurable. Codex CLI 0.167 accepts `lklimek/agents` as
  a marketplace but finds no plugins in it (Claude-format manifest only), so a marketplace install is not
  used.
- **Portainer** — the stack is plain compose services and named volumes; nothing creates containers at
  runtime in v1.

**v1 limit: public repositories only.** The shared runner has one cache and one credential store, so it
gives no isolation between tenants or users. Private repositories arrive together with per-job containers.

**GitHub access for the agent.** For each job the server mints a GitHub App installation access token with
read-only permissions (pull requests, issues, contents, metadata), restricted to the PR's repository and its
related repositories, with a fixed lifetime of one hour, and puts it in the `JobSpec`. The runner starts
GitHub's official `github-mcp-server` in read-only mode for the agent with that token. Related work is found
entirely by the LLM through this server. The GitHub App is installed on the target repositories.

**Related repositories** are detected by the server from `Cargo.toml` / `Cargo.lock` on the base ref (git
and path dependencies) and limited to repositories of the same GitHub organisation. v1 recognises Cargo
only; there is no manual configuration.

**Agent context and rules**
- The agent uses the `claudius` plugin directly (severity and system-rating rules, review skills) instead of
  copies of those rules in the prompt.
- Review lenses in the prompt: reuse of existing code, right layer / ownership, data design, behaviour at
  boundaries, performance and abuse.
- **Project profile**: risk is rated at system scale against the project profile (union of the base-ref
  file and MemCan memory; the file wins on conflict). Every rating opens with what a production user or
  operator would notice. Without a profile, ratings are component-level and the PR context says so.
- **Calibration memory**: feedback is stored locally and written to MemCan; `overrated` / `underrated` are
  saved as severity calibration examples that later analyses read through the MemCan MCP. MemCan scope:
  `project` = short name of the analysed repository. MemCan URL and token are configuration.
- Tests are not shown or walked through, but the agent may read them as evidence ("covered by test X").
- Deterministic, free signals from Not analysed material (failing CI tests, lines removed from tests,
  changed dependencies) are surfaced by the server; the LLM does not interpret that material in v1.

**Job execution interface (designed for per-job containers)**

Target model (later version): the server reads the tenant's config, starts one container per job with the
tenant's tokens, the container checks out the repository (or unpacks its cache), runs the agent and exits.
v1 must reach that model by adding one new implementation, without changing the pipeline or the runner logic:

1. `JobSpec` is self-contained: job id, kind, repo clone URL, base and head SHA, prompt, output JSON schema,
   limits, LLM provider, model and reasoning effort, MCP endpoints, credentials (the read-only installation
   token; LLM credential reference; a repo token later for private repositories), optional agent-session
   reference. Nothing is passed through shared volumes or
   ambient container state except the agent-session store, which is treated like a cache (item 5). In v1 the
   repo token is absent.
2. `JobExecutor` (server-side trait): `submit(JobSpec) → JobHandle`, `events(JobHandle) → stream`,
   `cancel(JobHandle)`. The pipeline and Ask only talk to this trait. v1 has one implementation,
   `QueueExecutor` (puts the job on the DB queue for the long-lived runner). A later `ContainerExecutor`
   starts a container per job. The server never assumes that a runner is long-lived or that two jobs share a
   filesystem.
3. Runner core is one function: `execute(JobSpec, EventSink) → JobResult` — fetch, checkout, run agent,
   clean up. The v1 polling loop is a thin wrapper around it; a one-shot entry point for per-job containers
   is a second thin wrapper, not built in v1.
4. Runner ↔ server protocol is the same in both models: authenticated HTTP — claim job, post events,
   post result. Each job carries its own short-lived token scoped to that job.
5. Caches are an optimisation only: a job must succeed with an empty cache. This includes the agent-session
   store: a missing or unusable session falls back to a new session (see Agent sessions). In v1 the store is
   a runner volume keyed by analysis; for per-job containers the session archive will travel with the job
   (stored by the server or on a session-store volume) — later work.
6. Per-job containers must not carry the stack's `com.docker.compose.project` label (Portainer attributes
   containers to stacks by it). Docker socket access goes through a restricting proxy. Both are later work.

**Job kinds**: `plan`, `pr_context`, `analyse_flow`, `ask`. Batch kinds form the analysis pipeline; `ask` is
interactive.

**Agent sessions.** Each analysis has one agent session. Its batch jobs (`plan`, `pr_context`, each
`analyse_flow`) and later `ask` jobs resume that session instead of starting cold, one job at a time per
session; at the end of every job the session is compacted. For a large PR the agent may delegate to
sub-agents at its own discretion (the prompt permits it; the server does not orchestrate them). If the session
is missing or unusable, the job starts a new session seeded with a context pack built from stored results
(PR context, plan, analysed flows; for Ask also the step, its flow analysis and earlier questions and answers
of that step). A re-analysis on a moved head starts a new session. If resume or compaction is not available
in non-interactive mode, every job is a new session with a context pack.

**Ask requires on-demand interactive jobs.** Ask is not part of the batch pipeline. A question about a step
becomes an `ask` job on the same `JobSpec` / `JobExecutor` interface; it resumes the analysis's agent session
and carries the question and the step. The job streams the answer through the same events channel; the
answer is stored as a `Question` of the step. Ask is available only after the analysis has finished, so it competes
with dashboard ranking jobs, and with batch jobs only during a re-analysis; an `ask` job is claimed before every
other queued job, ranking before batch (a running job is not pre-empted).
The job interface must not assume that all jobs are long-running batches.

**Pipeline** (each step is a resumable job; partial results are persisted but not shown until the analysis
finishes)
1. Ingest: PR metadata, threads, checks; fetch base and head into the server's bare mirror.
2. Pre-pass (deterministic, language-agnostic): hunks, classification through tech plugins (path globs and
   line regexes), moved-block candidates (`git diff --color-moved` + token-normalised hashes),
   whitespace-only changes, counters (lines removed from tests, changed dependencies), related-repository
   detection.
3. Plan: the agent partitions hunks into flows. Huge PRs: per-directory summaries first, then grouping.
4. PR context (own job): problem and why, related work through `github-mcp-server`, fit questions with
   evidence, project profile lookup. Independent of step 5; queued like any job.
5. Analyse: one agent run per flow — flow graph (structured), flow introduction, steps with called by /
   decides / next / why, annotations, assessment, out-of-diff impact, pattern repetitions, LLM status of the
   GitHub threads located in its steps. Flows classified as pure refactor or rename are analysed like any
   other flow.
6. Verify: schema, ranges within files, coverage of every changed line (uncovered lines become
   "Unexplained"), move claims, flow-graph consistency (edges reference existing nodes, every node maps to a
   step); up to 2 repair runs.

There is no summarise step: no PR-level summary or overall risk. The progress bar counts flows analysed
(step 5).

**Tech plugin** (trait): `detect(file)`, `classify(file) → ranges of test / comment / doc / generated`,
`call_hierarchy(symbol) → Option<callers, callees>`; later `symbols`. In v1 `call_hierarchy` returns `None`
(TODO: static analysis such as tree-sitter tags in the language plugins; no rust-analyzer, it needs too much
RAM). v1 ships `generic`, with built-in "not analysed" rules of two kinds:
- path globs: `tests/`, `*_test.go`, `*.md`, lockfiles, `// Code generated` headers;
- line regexes: for example lines matching `^\s*//` (comment-only lines).
Custom rules come later. Rust and Go plugins come in v2; until then test code inside production files is
LLM-classified (§3).

**LLM provider** (trait): `run(prompt, cwd, json_schema, model, limits) → event stream → {structured,
session_id, usage}`, `resume(session_id, …)` with the same result, `cancel`; `run` and `resume` end by
compacting the session. Credential types: `codex_chatgpt` (subscription; owner's tenant only) and
`api_key` (required for other tenants: Anthropic forbids subscription tokens in multi-user services, OpenAI
is unclear). See `llm-runtime.md` (Background material).

**Model selection** — a single plain Rust function in the server: job kind and context in,
model and reasoning effort out. It reads the installation-wide Settings choice, defaulting to Sol/high.
Any signed-in allowlisted user may save one of the server's fixed model/effort options. The choice applies
to newly submitted jobs, including remaining jobs of running analyses; stored specs record actual usage.

**Comments** — the server writes to the user's GitHub pending review with the user's token: the first
comment creates the pending review, later ones are added, an existing pending review is reused; edit and
delete act on GitHub; the user submits on GitHub. Expected API: GraphQL `addPullRequestReview` (pending) and
`addPullRequestReviewThread`. A comment on a line outside the PR diff is added as a file-level comment whose
body links to the line. A comment on a changed line without annotation is also saved as feedback
`missed by the LLM`. The pending-review count in the top bar and the list in the centre are read from GitHub.

**Multi-tenancy** — every table carries `tenant_id`; `tenants`, `users`, `memberships` and tenant settings
exist from v1 with one seeded tenant `default` (allowed sign-in logins: `lklimek`). No tenant UI.

**Security** — PR content is untrusted (prompt injection): read-only agent, no repo-provided agent config,
explicit egress proxy that logs every destination host, only the LLM credential and the job's read-only installation
token in the runner. Sign-in is limited to the logins the tenant allows. Agent-session files are agent-writable and persist across the jobs of one analysis only; they are
stored per analysis and never shared with another analysis.
Accepted risks in v1: the agent has full MemCan MCP access (read and write) with the shared MemCan token, so
an injected PR could write misleading memories (until MemCan can distinguish agent-written memories); and
agent commands have open network access through the proxy (allow-all policy), so an injected PR could
exfiltrate MemCan content or the per-job Codex credential. Feedback and calibration examples are written by
the server. MemCan is reached at its Traefik entrypoint, which enforces an IP allowlist (it must include the
address the egress proxy presents) and a bearer token. Secrets encrypted at rest. Cookie sessions, CSRF, secure-cookie
and trusted-proxy settings so the app can sit behind a TLS reverse proxy.

### Frontend structure

Rule: every block on a screen is a self-contained component that can be placed on another screen without
change. It receives its data and callbacks as inputs and does not know which screen hosts it. Screens only
arrange components. This is mandatory structure, not a suggestion. Purpose: moving the Groups workspace or
other layouts in later is a layout change, not a rewrite.

Components: navigation list (left menu), PR context, flow introduction, flow diagram, step block (four facts
+ code), code view (line renderer with gutter marks and highlights), inspector and each of its sections, Ask,
feedback buttons, comment composer, pending-review list.

## 6. Technology

Chosen to match the owner's active projects (penny, gamiscreen, memcan). sqlx (penny) and diesel
(gamiscreen) are tied there; sqlx wins as async-native with axum.

| Concern | Choice |
|---|---|
| Server | Rust, tokio, axum 0.8, tower-http, tracing, thiserror, clap, dotenvy |
| Database | sqlx with SQLite only. All database-dependent code (queries, migrations, row types) lives in one module behind a repository interface; nothing else touches sqlx. Moving to PostgreSQL later is a rewrite of that module, not a supported switch. |
| API types | serde; `schemars` (JSON schema for agent output), `jsonschema` (validation), `ts-rs` (TypeScript types) |
| GitHub | GitHub App: user access tokens for the user's actions, installation access tokens (from the App private key) for the agent; `octocrab` or `reqwest`; GraphQL for the pending review |
| Secrets | `aes-gcm` |
| MemCan | `rmcp` client (feedback writes); the agent uses MemCan MCP directly (read and write) |
| Agent tools | `github-mcp-server` (official, read-only mode) started by the runner with the job's installation token |
| Diff / moves | `git` subprocess, `similar` |
| Frontend | React 19, TypeScript, Vite, TanStack Query, react-router, Tailwind 4; code shown by an own line renderer (plain HTML rows) with Shiki for syntax colouring, no editor component; flow diagram rendered by the UI from the graph data; vitest, Playwright; embedded with `rust-embed` |
| LLM | Codex CLI (`codex exec`) as a subprocess; no SDK, no third-party crates |
| Packaging | docker compose: `server`, `runner`, `egress-proxy` (Squid); named volumes |

Exact versions are pinned at implementation time from the registries.

## 7. Large files

Measured 2026-11-16 on dashpay/platform, tenderdash and dash-evo-tool: large files are routine. In
dash-evo-tool 45% of recent changes touch a file over 1000 lines (8% over 5000); platform has hand-written
files of 14–16k lines and generated gRPC clients of 135–140k lines.

Rules:
1. The UI never renders a whole file; the Tour shows steps. The API serves file content by line range.
2. Shiki colours only the displayed lines; occasional wrong colours (a step starting inside a multi-line
   comment or string) are accepted.
3. Generated files are Not analysed by path rule before any agent run; never rendered, never given to the
   agent.
4. The agent reads line ranges and searches; prompts tell it not to load large files whole.

## 8. Open points
Only what is not decided:
- GraphQL pending-review API (`addPullRequestReview`, `addPullRequestReviewThread`): verify behaviour, reuse
  of an existing pending review, and edit/delete support at implementation.
- Codex's own sandbox (bubblewrap/landlock) inside an unprivileged container: verify it works or must be
  disabled in favour of the container sandbox.
- Exact Claude and Codex flags and settings that neutralise repository-provided agent configuration
  (instruction files, hooks, MCP and plugin config in the checkout).
- Portainer handling of containers created outside the stack (the later per-job model).
- Codex in non-interactive mode: session resume from a fresh work dir, the equivalent of `/compact`, and
  sub-agents — verify.
- Pending comments that are file-level (lines outside the diff): verify the mechanism.
- `github-mcp-server` with an installation token: accepted, and able to read public repositories where the App
  is not installed — verify.

## Background material

Outside the repository, in `/data/artifacts/diffpilot/2026-11-16/`:
- `research/prior-art.md` — survey of existing review tools and the base shape of the candidate JSON (§5).
- `research/llm-runtime.md` — running Codex and Claude as an agent runtime; credential constraints.
- `reviews/psych-astra.md`, `reviews/psych-fable.md` — two psychology reviews of the first mockups.
- `input/review-assist.md` — the owner's manual-review skill.
- `mockups`, `mockups-v2`, `mockups-v3`, `mockups-v4` — archived earlier mockup sets, including the dropped
  Groups workspace, overview and files screens. Superseded by `docs/mockups/`.

### Owner-approved amendment — 2026-11-17: show less in the Tour

Empty explanatory sections are hidden, without implying safety. Unavailable deterministic checks retain one factual line. The navigation owns the step list and kind icons; the Inspector owns step facts, caller/next descriptions and the collapsed assessment. The decision sentence belongs under the step title and in the diagram node's accessible description. Evidence remains attached to each statement, collapsed beneath it behind “Evidence · N places”; opening it exposes short in-app line links. The code occupies the full centre width. The diagram uses the persisted step order from top to bottom, independent of viewport width, so navigation, diagram, Previous/Next and deep links agree. Replacements pair old and new lines under one label; pure deletions remain distinct.

Test files, fixtures, mocks, benches and test helpers recognised by deterministic path rules are outside LLM planning, flow analysis and coverage. They appear in Tests once with file and added/removed-line counts and access to the diff. Test code may be read as evidence for a production step and mentioned in one clause; never create a test-subject flow. Inline Rust test modules/functions are not yet delimited by the generic plugin; a Rust syntax plugin is required before excluding these ranges deterministically.

Rust comment-only lines (including block/doc comments) and complete single-line `use`, `mod` and `extern crate` declarations remain available to the agent as context, but require no explanation, annotation, assessment or flow. They are covered by a deterministic rule. Main code views hide them by default with a per-user browser preference and a hidden-line count; the classic diff always retains them. Other languages have no such rule. A contradiction between a comment and production code may be cited on the code's step. This supersedes the earlier test-flow and comment-omission rules above.

### Owner-approved amendment — 2026-11-18: nothing behaviour-changing is hidden

The rule above covers only plain material, read from the Rust lexer's tokens (never raw text); what the rule cannot read with certainty is not covered. Covered (context: hidden in the main view by default, sent to the LLM, no coverage duty): `//`, `///`, `//!` lines; block comments when every line they span changed and every changed line of their hunk, on both sides, is comment; a single-line private `use` without `as` or `*`, alone on its line at the top level of the file after `;`, `}` or the file start, binding only names that no other `use` of either side binds to another path, that no item of the file declares, that are not prelude names and that the other side either binds by a plain `use` of the same path (a moved or regrouped import) or uses nowhere except as a `use` binding (so an extern-prelude crate, a `#[macro_use]` macro, a derive, an attribute or a constant in a pattern cannot be re-pointed unseen), in a file with no glob import on either side. Whitespace-only lines, as GitHub's "hide whitespace" shows them: a changed Rust line whose lexer tokens equal those of its exactly paired changed line on the other side (only indentation, trailing whitespace, line endings or spacing between tokens differ; punctuation jointness counts, so `a - -b` vs `a--b` stays changed; never a line touched by a multi-line string, a block comment or a macro token tree) is shown in the main view as ordinary unchanged code: no change mark, its base counterpart not shown as removed, not behind the switch and not counted as a changed line; it is sent to the LLM as context without coverage duty. Re-indented code inside a new `if` reads as unchanged while the wrapper lines are marked; pairing work is bounded per file, beyond it the lines stay changed. Never covered (shown, and in a flow or Unexplained): `mod name;` and `extern crate` (they add or remove whole files or crates); visibility-qualified or attributed declarations, also when the visibility or attribute is on the previous line; `use` inside a function, block or inline module; `as` aliases; globs; any line inside a macro token tree (`name! { … }`, `r#name! { … }`, `macro_rules!` bodies); block-comment delimiters that switch code off or on; anything when the other side was not read. Reading a finished analysis again under these rules never rejects its stored plan or flows: changed lines they do not cover are listed under Unexplained. Identical moves are quiet rows only when they leave a file that still exists at head for another file, or as a pure rename; their lines are sent to the LLM as context with the move fact. A reorder within one file, renamed or not, and a move out of a deleted file (possibly a rename git did not pair) stay with the agent and keep their move fact. The LLM is asked to cover a context line when it changes behaviour. Residual risk: a covered plain `use` (trait method resolution), a doc comment read by a derive macro or run as a doctest, or a quiet move can still change behaviour; only the LLM, asked to speak up, sees them. A source file left as a stub after its code moved to a new file still gives a quiet move. A renamed file with changed lines on both sides counts as two paths in the Unexplained file count. Inline Rust `#[cfg(test)]` code in production files stays with the agent until a syntax plugin delimits it.

### Owner-approved amendment — 2026-11-18: provenance tags, Ask below the field, collapsed context code

- **No-questions sentence.** "The LLM raised no questions. That is not evidence of no defects." is no longer
  rendered; an empty Questions section is hidden like every other empty explanatory section. Supersedes that
  sentence in §1 (Silence is not safety) and §4.3 (Inspector, section 6).
- **Provenance tags.** The caption "LLM analysis unless marked otherwise" is removed everywhere. Explanatory
  text written by the LLM (sections, the decision sentence, pattern descriptions, thread status, marker
  popovers) carries a small "LLM" tag (tooltip: written by the LLM, not checked); LLM-written titles and labels
  carry no tag and keep their own labels and tooltips (§3: no badge means LLM analysis). Content taken from
  GitHub review threads carries a "GitHub" tag. A tag names the source, never correctness; deterministic facts keep
  their fact badges. The step's decision sentence under its title carries the "LLM" tag. Supersedes the
  one-line-per-panel rule in §1 (A badge names exactly what was checked) and the caption in the flow
  introduction block of §4.3.
- **Ask.** Answers render below the question field, newest first, with pending and failed states in the same
  place; each keeps its LLM answer label and its author label ("Asked by you"). Its tooltip says the agent was told
  not to run code, never that nothing was executed. The answers block has its own scroll in the dock, so
  Feedback and Comment stay in view. Supersedes "answers stay pinned to the step" (§4.3 Dock, Ask) as to their
  place.
- **Context code.** In the context part of a step (impact outside the diff), each code block starts collapsed
  behind a one-line header naming its file and lines; its explanation stays visible. The expanded state is not
  carried to another step. Supersedes "impact outside the diff inline under the step as real code" (§4.3 Step
  block).
- **Open.** The link that goes to an existing review reads "Open", whether or not the user has started it, on
  Home, the progress screen and the dashboard. It only navigates: its tooltip says it starts nothing (no
  analysis, no LLM use) and, for a started review, that ticks and answers are kept. On the progress screen it
  sits apart from the action buttons, as a navigation link with an arrow. "Resume" is only the button that
  continues a paused analysis. Supersedes "Resume review" and "Open review" of §4.1 and §4.2.
- **Comment drafts.** An unsent comment text is kept per line in the browser tab (sessionStorage, per user login
  and analysis) so navigation and a reload do not lose it. It is dropped only when the comment is added, when its
  text is cleared, or on an explicit sign-out by that user (that login's drafts). An expired session or a 401
  keeps it (also a comment refused as unauthenticated), and the same user signing in again on the tab gets it
  back. A post whose outcome is unknown is kept the same way with its idempotency key and comes back locked
  ("may already be in your pending review"): Retry sends it with the same key, so it cannot be added twice;
  Discard makes the text editable again. The composer's Close and Escape keep the draft. Supersedes "No local drafts" (§2 v1, Comments) and "Local comment drafts" (§2 Non-goals).

### Owner-approved amendment — 2026-11-18: explicit start, flows on request, sign-out

- **Start analysis.** On Home, Enter or "Check" only looks the PR up (busy state on the field). The result
  appears as its own card under the field, announced and scrolled into view: PR title and number, the size
  warning with its estimate when it applies, an existing analysis with "Open", or the
  lookup error. Nothing starts without the card's primary button "Start analysis" (it takes the focus, so a
  second, deliberate Enter starts it; a held key or one in the moment the card appears does not); with a size
  warning or without commenting it reads "Start analysis anyway" below the warning, and the card's findings
  describe it; "Cancel" clears the card, and so does any edit of the field. A moved head is started the same way.
  After a cancel Home says "Analysis cancelled. Check the PR to start again.". Supersedes the start on "Analyze"
  and "Re-analyze", and "Analysis cancelled — analyze again", in §4.1 and the separate "Analyze anyway" step.
- **Language of an analysis.** Wherever an analysis is offered or shown (Home's check card, the progress screen
  and Tour top bar) a small tag names the language of its LLM texts ("EN" / "PL" / "RU"). An existing analysis in
  another language than the user's setting is still offered first ("Open"); a secondary
  "Analyse again in <language>" starts one in the user's language.
- **Flows on request.** Flows are analysed only when the user asks: "Analyse this flow" on a flow not analysed
  yet (opening it requests nothing), or "Analyse all remaining flows", the one opt-in for everything (the
  analysis's background switch; shown as "Analysing all remaining flows — Stop" while on), in the Tour's left
  panel and on the progress screen (which shows the number of remaining flows only, no changed lines). The web
  reads the switch from the analysis and assumes no default. With
  nothing queued or running: "Nothing is being analysed. Choose a flow to analyse, or analyse all remaining
  flows."; the progress screen says the plan is ready and offers "Open". Supersedes "Opening a pending
  flow requests it" and "the background switch defaults on" in the 2026-11-17 amendment of §4.2.
- **Sign-out.** Settings has "Sign out": it ends the session in this browser (`POST /auth/logout`; a session
  that had already ended counts as signed out), removes that login's comment drafts and unconfirmed posts from
  the tab, clears cached data and shows the signed-out Home.

### Owner-approved amendment — 2026-11-18: code-view colours, remark bars, folds, collapsible Ask

- **Change colours.** The code view draws changes exactly like the classic diff, in both themes and from one
  shared definition: an added line has the classic diff's green fill and `+`, a removed line its red fill and
  `−` (deterministic, from the diff). A moved line is added code in the classic diff and has the same green
  fill; its double bar says it moved. Supersedes "neutral mark on every changed line" (§3 and §4.3 Code view),
  the legend row "Neutral gutter bar = changed line" and the muted fill of moved blocks.
- **Remark bars.** An LLM remark never fills code lines and draws no box: its range is a thin bar in the
  reserved gutter column (solid / dashed / dotted by the LLM's rating, not checked; a thin neutral line when
  unrated) with one label at its top and one `◆` marker. Unchanged lines inside the range look exactly like
  unchanged lines outside it; changed lines inside keep their green/red look. A range that crosses a replaced
  or removed block is one range: the label sits above the block, so the removed and replacing lines stay
  together. "A changed line without a bar has no LLM remark." Supersedes "LLM colouring is laid on top" (§3)
  and "No colour on a changed line" in the §4.3 legend. The `?` legend describes exactly these drawings.
- **Folds.** Hidden comment and import lines leave a fold row where they were ("3 comment/import lines hidden ·
  Show"; removed ones "2 removed comment/import lines hidden"; the diff does not say which line is which); Show opens just that run in place with its signs and
  offers Hide; the top-bar switch still shows or hides all; the total note stays. Only the first line of a
  remark range is never hidden (it carries the marker); the rest may fold inside the bar.
- **Ask** is a collapsible Inspector section like the others (remembered with them; the header keeps the
  step's question count); `a` opens it and focuses the field. (Its caption: see "Ask as a chat" below, which
  supersedes "answers appear below the field, newest first".)

### Owner-approved amendment — 2026-11-18: LLM rating and remark pills in navigation

- On the wire a remark has no severity of its own; only its step has the LLM's rating. A pill shows the step's LLM
  rating only ("▲ HIGH"; name "LLM rated this step HIGH (not checked)"); the number of its LLM remarks is in the
  tooltip ("5 LLM remarks on this step"). Remark counts are never split by rating (amended 2026-11-18, F27).
- The navigation shows one pill per step and per flow row (also collapsed); the flow introduction and the step
  (above the code) show the same. A flow's rating on the wire is its highest step rating, already shown by the flow's
  LLM label, so a flow with remarks gets a small neutral marker (a dot, name "11 LLM remarks in flow 2"); its
  tooltip has the remark count and lists the steps per rating.
- Remarks on an unrated step get the same neutral marker ("5 LLM remarks on step 3"); a rated step without remarks
  shows its rating alone; a step with neither shows nothing, never an all-clear. Remarks with the user's feedback
  dim the pill.
- A pill leads to the first remark (on a step at the highest rating for a flow) and flashes its code line and
  Inspector entry. `r` goes to the next step with remarks after the current position (from a flow introduction:
  that flow's first step), wraps around, and says "No more steps with remarks" when there are none.
- Amends §1 "badges only for deterministic facts": these pills are LLM judgements and are allowed in navigation
  because they are styled as LLM content (the LLM tint, the remark bar's colour and pattern, a word label) and say
  "LLM" and "not checked" in their accessible name.

### Owner-approved amendment — 2026-11-18: Ask as a conversation

- **Ask as a chat.** The Ask area shows the user's conversation of the analysis as one thread, oldest at the top
  and newest at the bottom, the field below it. Each entry is the question with the step it was asked on (a small
  link that opens that step) and its answer; questions asked on other steps belong to the same thread: it is the
  user's conversation about this pull request's analysis. "Whole conversation" (default) and
  "This step" filter it. The thread follows the newest entry unless the reader scrolled up; a new question always
  shows. Pending answers keep their progress line, failed ones Ask again. Answers keep their line breaks; list
  lines are shown as typed with a hanging indent. Supersedes "Answers render below the question field, newest
  first" above.
- **Resizable Ask.** A horizontal handle on the dock's top edge (`role="separator"`, its height as value) makes
  the Ask area taller by dragging up (the Inspector sections above shrink, keeping room for the heading) or with
  the arrow keys; Home and End go to the limits, Enter or double-click resets. The height is remembered with the
  tour layout like the diagram's. A collapsed Ask has no handle and its natural height.
- **Ask about a line.** Each line of the step's code view and of the classic diff has a small "?" action (shown on
  hover and on keyboard focus; Tab from the line's comment action reaches it, arrow keys move between lines). It
  puts `path:line` (`path:start-end` when the line is inside a selection of several lines; " (old code)" for a
  removed line) into the Ask field at the cursor, opens Ask and focuses the field; nothing is sent.
- **Use as review comment.** A flag card and an Ask answer asked on the open step offer "Use as review comment"
  (only while commenting is available). It opens the existing comment composer: a flag on its line range, an
  answer on the open comment's line, else the step's first changed line; placement falls back as for any
  comment (file comment, not in the diff). The draft starts with "LLM-suggested — review before posting", then
  the flag's title, numbered scenario and not-checked line, or the answer as written; a draft already there keeps
  its text and gets this after a blank line. It stays an editable draft; nothing is posted until the user adds it.

### Owner-approved amendment — 2026-11-18: GitHub work dashboard

The start page `/` is "My work": the pull requests and issues the user authored, is assigned or is asked to review,
and optionally the user's MemCan TODOs, ranked P0–P3 by a computed score with visible components, plus
pull-request throughput charts. It refreshes on request and once a day at a time the user sets (default 06:00),
and is read-only towards GitHub and MemCan. The user's agents may read it, and everything else the user can read,
with a personal access token. The review start page moves to `/review`. §1 trust rules apply to the review
module; on the dashboard LLM text carries the "LLM" tag and every score part names its source. Supersedes "No
dashboard" in §2 v1 and the v3+ dashboard item in §2 Later. Decision: `docs/decisions/ADR-016-dashboard-start-page.md`;
plan: `docs/v1.1-dashboard-plan.md`.
