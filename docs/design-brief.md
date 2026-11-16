# smart-review — design brief

Status: design approved pending final mockup sign-off, 2026-11-16. Single source of truth for the design;
requirements, ADRs, architecture and mockups derive from it. Implementation is a later phase.

## 1. Product

**Problem.** A classic diff does not tell a reviewer what happened: moved code shows as removed + added,
production code and tests are interleaved, and nothing explains what the change does or whether it is the
right change for the project.

**Product.** A self-hosted web tool that helps a HUMAN review a GitHub PR. It shows the FINAL code with
changed regions highlighted, grouped into flows, and explains each step: what changed in behaviour, why,
the consequences, the call hierarchy, questions to check and a system-scale risk assessment.

**Purpose beyond the diff: avoid the local optimum.** A PR can be correct in isolation and still be the wrong
solution for the project (duplicate mechanism, wrong layer, competing PR, oversized for the problem). The tool
gives the reviewer the PR context (problem, related work, fit questions, CI, project profile) so the reviewer
can judge the PR at project scale, not only line by line.

**Principles**
0. Keep it simple: the simplest design that meets the scope; no speculative abstraction. Prefer removing or
   rewording over adding UI.
1. Not an automated reviewer. The human decides; the tool directs attention.
2. Attention is the scarce resource: the most important thing is visible first; quiet material is collapsed
   and never ranked above a rated group.
3. Final code, not old-vs-new. The old version is available on demand per fragment (deleted code is the one
   exception: shown by default at the deletion marker).
4. Every changed line belongs to exactly one group (coverage invariant); lines the agent failed to analyse go
   to "Unexplained — read these yourself", listed first.

**Trust rules** (a wrong "pure refactor" or "no risk" label hides bugs; wording is part of the design)
- **A badge names exactly what was checked.** Only deterministic facts get a badge: "text identical",
  "classified by path", "classified by line rule". Everything else is LLM analysis; one line per panel says
  "LLM analysis unless marked otherwise". The blanket word "verified" is not used. A green tick is only for
  deterministic content checks.
- **Silence is not safety.** Never "no risk", "nothing to review", "no review needed". Material the tool does
  not analyse is "Not analysed", not "noise". When the LLM raises nothing: "The LLM raised no questions. That
  is not evidence of no defects."
- **Verdict after evidence.** Evidence (behaviour, why, consequences, call hierarchy) comes before
  assessment; assessment is collapsed until clicked. LLM suspicions are phrased as questions to check, not
  verdicts. The hover tooltip states only the was → is fact.
- **No numeric confidence** anywhere. Observed facts carry a line reference; inferences are marked as
  inferences; hedges survive shortening. Call hierarchies are stated as a search result, not a complete list.

## 2. Scope

### v1
| Area | Decision |
|---|---|
| Input | One field: GitHub PR URL. Public repositories only. No dashboard, no recent-reviews list. |
| Users | GitHub login (GitHub App, user access tokens). Single tenant `default`, hardcoded. |
| Views | Two: group workspace (default after opening a PR) and tour. No overview, no files tree. |
| PR context | Problem and why, related work, fit questions, CI, deterministic counters, project profile status. |
| GitHub context | Existing review threads at their fragments, with an LLM status; CI status. |
| Ask | Free-text question about the selection, answered by the agent with repo access, pinned and labelled as an LLM answer. No one-click deepen actions. |
| Progress | Tick off fragments and groups; top-bar counter "groups reviewed k / n"; persisted per analysis. |
| Comments | On any displayed line; written straight into the user's GitHub pending review. No local drafts. |
| Feedback | correct / wrong / irrelevant / overrated / underrated + note; "missed by the LLM" recorded automatically. Stored locally and written to MemCan. |
| Resume | Opening a PR with an existing analysis resumes it (ticks, answers kept). If the head moved: banner and "Re-analyze" (fresh analysis, ticks not carried over). |
| Language | UI English. LLM texts in the user's configured language (default English). |
| Languages of code | Any, LLM-only. Built-in `generic` tech plugin (path globs and line regexes). |
| PR size | No limit. Huge PRs (10k+ lines) are split into many agent runs; long runtime is acceptable. |
| LLM | Codex CLI on ChatGPT subscription. Provider trait designed for a second provider. |
| Agent access | Read-only checkout, no code execution. MemCan MCP, the `claudius` plugin, a server-side read-only GitHub lookup tool, related repos as extra context. |
| Deployment | docker compose, LAN. Portainer-compatible. Designed to be exposed publicly later. |

### Later
- v2: re-review (semantic delta since the reviewed head, ticks carried over); recent-reviews list; LLM
  "auto-comment" (drafts an English PR comment); Claude provider; Rust and Go tech plugins (tree-sitter
  test/comment ranges, LSP call hierarchy); PR-description-vs-code check.
- v3+: LLM-curated dashboard of PRs to review; per-job containers (tenant tokens, private repositories,
  later build/test).
- Later, unscheduled: static-analysis actions (find callers, test coverage); sync of ticks with GitHub
  "Viewed"; custom "not analysed" rules.
- v5+: real tenants (tenant = organisation, own LLM keys, users ↔ tenants many-to-many), billing.

### Non-goals
Automatic approve/reject. Publishing reviews without the user. Replacing CI. Local comment drafts. PR-level
overall risk score or size statistics. A setting to show or hide verdicts.

## 3. Domain model

```
Tenant 1─* Repo 1─* PullRequest 1─* Analysis (one per head SHA + language)
Analysis 1─1 PrContext
Analysis 1─* Group 1─1 FlowGraph 1─* Fragment (= flow step) 1─* Annotation
Group    1─* PatternRepetition
Analysis 1─* NotAnalysedSection 1─* Item (path- or line-rule matched; moves with identical text)
Fragment 1─* ImpactRef (code outside the diff);  Fragment 1─* Question (Ask thread, LLM answers)
User *─* Tenant;  User 1─* ReviewState (per analysis): ticks on fragments and groups
User 1─* Feedback (→ MemCan);  Feedback may reference a Fragment, an Annotation or a plain changed line
```

Comments are not stored locally; they exist only in the user's GitHub pending review.

**PrContext** — the problem the PR solves and why; related work (each item: link, kind — duplicate,
competing solution, stacked PR, linked issue, companion repository — and one line on the overlap); fit
questions with evidence (right layer / repository? oversized for the problem? duplicates an existing
mechanism?), phrased as questions, not verdicts; CI status (failing check and test names); counters from the
deterministic pre-pass ("N lines removed from tests", "M dependencies changed"); project profile status
(found with source, or "no project profile — ratings are component-level").

**Group** — one flow: a logical change following control flow (trigger → calls → state changes → result)
across files, including unchanged code that connects the changes. Fields: title, classification, importance
(ordering), problem statement (trigger, what was wrong or missing, intended correction, short before/after
example), flow graph, tour order, dependencies on other groups.

Classification (closed list): `behavioural`, `api_contract`, `new_code`, `refactor_pure`, `move`, `rename`,
`test`, `docs_comments`, `generated_mechanical`, `config_build`, `dependency`, `other`.
`test`, `docs_comments` and `generated_mechanical` material forms the Not analysed sections.

**FlowGraph** — structured data, not diagram syntax; the UI renders it. Nodes: id, step number, kind
`added` / `changed` / `unchanged` (unchanged = context), fragment reference, label. Edges: from, to, branch
label (decision outcomes, loop back-edges, error exits). Each node is the diagram twin of one fragment.

**PatternRepetition** — a pattern applied in several places is explained once (in a fragment); the other
places are listed as a collapsed row "same pattern × N", each with a fragment reference.

**Fragment** — one step of a flow: a line range of one file at the head commit, numbered like its diagram
node. Fields: file, symbol, GitHub line link (head SHA), called by, decides/changes, next (where execution
goes), why (why this step or modification is needed), kind, annotations, collapsed assessment.

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

**Assessment** (per fragment, collapsed by default): headline = what a production user or operator would
notice; risk level at system scale with reason (also for LOW); basis ("read 2 files, nothing executed") and
what was not checked; introduced by this PR or pre-existing; blocks this PR or follow-up; smallest correction.

**Annotation** — anchored to lines inside a fragment. Types: `behaviour` (was → is), `consequence`,
`question` (questions to check, including LLM suspicions of bugs), `assessment`, `note`. Each carries
evidence references (file:line). Observed facts are marked as facts; inferences as inferences.

**Call hierarchy** — callers and callees per fragment symbol, each tagged with `source` (`llm` in v1; `lsp`
later) and a GitHub link. **Provenance badges** (deterministic only): `text identical` (moved block equal
after whitespace normalisation), `classified by path`, `classified by line rule`; `git color-moved` is
internal evidence for move candidates. No badge means LLM analysis.

**Feedback** — values: `correct`, `wrong`, `irrelevant`, `overrated`, `underrated`, `missed by the LLM`,
plus optional note. `missed by the LLM` is created when the reviewer comments on a changed line that has no
annotation. `overrated` / `underrated` also become calibration examples in MemCan.

**Candidate JSON** emitted by the agent: base shape in
`/data/artifacts/smart-review/2026-11-16/research/prior-art.md` §5; the full schema is produced at
requirements time (`schemars`). Fields: per group — title, classification, importance, problem statement,
flow graph, fragments, pattern repetitions, dependencies; per fragment — file, range, symbol, kind, called
by, decides, next, why, annotations with evidence references, call hierarchy, impact refs, assessment; per
PR (PR-context job) — problem, related work, fit questions. No confidence number, no overall PR risk.

## 4. Screens

Rules for every screen:
1. Use the full page width (the owner works on a 21:9 monitor); no narrow centred column.
2. The PR number in the top bar opens the PR on GitHub in a new tab.
3. Every label (risk level, classification, provenance badge, status) has a tooltip explaining it.
4. Top bar: repo, PR number and title, head SHA, CI status, view switch Workspace / Tour, counter "groups
   reviewed k / n" (no percentage, no risk weighting), user menu.

States every screen needs: loading, empty, error, partial analysis, stale analysis (new head SHA exists).

### 4.1 Home
One field (PR URL) and "Analyze"; signed-out state "Sign in with GitHub". A PR with an existing analysis is
resumed (ticks and answers kept). If the head moved: banner "This analysis is for `abc123`; the PR is now at
`def456`" with "Re-analyze" (fresh analysis, ticks not carried over).

### 4.2 Analysis progress
- Stages: fetch PR → checkout → pre-pass → plan flows → PR context → analyse group k/n → verify. Groups
  appear as soon as planned and fill in as they complete; the whole group card is clickable and opens that
  group in the workspace. A group whose analysis exists is "analysed" (not "done" or "ready").
- Pre-pass result worded "Recognised by file path, no LLM: Tests (4 files), Docs & comments (3 files),
  Cargo.lock".
- Activity: short live lines of what the agent does (stage, file read, search), errors, usage-limit pauses.
  Raw agent log on demand.
- Lines the agent failed to analyse: "Unexplained — read these yourself", sorted first.
- States: running; paused ("Codex usage limit reached, resumes at HH:MM"); failed chunk with retry.

### 4.3 Group workspace (default screen)
Three columns: Groups | Code | Inspector. Each column has "hide" and "full screen". The inspector is empty
until the user clicks a highlight or a step; nothing is pinned automatically. Inspector sections are
collapsible; the collapsed state is remembered across fragments and files (browser localStorage).

**Left column**, in order:
1. PR context (first item; contents as in §3).
2. Flow groups, most important first (title, classification badge, risk, files, tick, existing-threads
   count).
3. Quiet rows, always last: moves with identical text; Not analysed sections (Tests, Docs & comments,
   Mechanical & generated) with the deterministic counters.
4. Pending review panel: comments read from the user's GitHub pending review; link "Finish on GitHub".

**Centre column**
- Group header: the problem statement. Flow diagram: numbered steps, decisions with labelled branches,
  loops, error exits; added, changed and unchanged-context nodes differ by colour AND a second signal
  (border style or marker); clicking a step scrolls to its code.
- Fragment cards (one per step, numbered like the diagram node): file, symbol, GitHub line link (head SHA,
  new tab). Final code only, except deleted code. Visible at once: called by, what it decides or changes,
  where execution goes next, **Why**. Collapsed until clicked: Assessment and recommendation. "Show old
  code" expands an inline classic diff; "mark reviewed" ticks the fragment. Impact outside the diff stays
  inline under the fragment, as real code. "Same pattern × N" collapsed rows for repeated patterns.
- Gutter: neutral mark on every changed line; LLM colouring only on annotated ranges; markers for
  annotation, deleted site, GitHub thread. Hover on a highlight: was → is only, no risk, no verdict.

**Inspector**, sections in order:
1. Classic diff of the selected range.
2. Behaviour (was → is).
3. Why.
4. Consequences.
5. Call hierarchy — "Callers found" / "Callees"; entries link to file and line on GitHub (new tab); stated
   as a search result.
6. Questions to check — LLM suspicions phrased as questions ("Can `broadcast_tx` be sent twice when the
   reply arrives late?"). If none: "The LLM raised no questions. That is not evidence of no defects."
7. Assessment (collapsed by default): fields as in §3.
8. GitHub threads at this location, each with an LLM status: still valid / stale / already addressed.
9. Ask — one-line text field that grows while typing; answers stay pinned, labelled as LLM answers.
10. Feedback — correct / wrong / irrelevant / overrated / underrated, optional note.
11. Comment — on ANY displayed line, annotated or not. "Add to GitHub pending review" writes straight into
    the user's pending review on GitHub.

A moved block with identical text reads "Body text identical; call site changed in group 1".

**Mockup must show three hard cases as switchable states:** (1) a confident but wrong HIGH, marked
wrong / overrated; (2) a defect in a plain changed line without annotation, commented by the reviewer and
recorded as "missed by the LLM"; (3) a PR where the LLM raises no questions.

### 4.4 Tour
- Under the title: what the change gives and why it is needed.
- Steps follow control flow (the same flows as the groups), including deletions. One fragment and its
  explanation per step, next/previous.
- No ticks. A link jumps to the highest-rated question.
- Not analysed material is skipped and listed as skipped.

### 4.5 Settings
- LLM credentials: "working" / "not working" only.
- Language of LLM texts.
- Per repository, read-only: built-in "not analysed" rules (path globs and line regexes), related
  repositories, project profile source.
- GitHub App installation status.

Keyboard: `j`/`k` fragment, `n`/`p` group, `x` mark reviewed, `d` old code, `c` comment, `a` ask, `?` help.

## 5. Architecture

```
browser ── HTTPS ──> server (axum: JSON API + SSE, embedded React SPA, orchestrator, SQLite, MCP lookup tool)
                        │  job queue (DB)            │ GitHub API (user token)
                        ▼                            ▼
                     runner (Rust binary + codex CLI, git, ripgrep)  ── egress allowlist ──> LLM, MemCan, server MCP, GitHub (git fetch)
                        own checkouts: per-job work dir + local mirror cache
```

- **server** — API, sessions, GitHub integration, own bare mirror (file content for the UI, pre-pass),
  deterministic pre-pass, validation of agent output, persistence, SSE progress, pending-review writes,
  read-only GitHub lookup MCP tool. Trusted.
- **runner** — a long-lived compose service, no Docker socket. Takes one job at a time per LLM credential
  (Codex rotates its token on refresh). For each job it fetches the repository itself (local mirror cache),
  checks out the head into a fresh work dir, runs `codex exec --json --output-schema` there, streams events
  back and deletes the work dir. `CODEX_HOME` on a volume, isolated from repo config. The container is the
  sandbox: non-root, network limited through an allowlist proxy service. It holds no GitHub token and no
  database access.
- **Portainer** — the stack is plain compose services and named volumes; nothing creates containers at
  runtime in v1.

**v1 limit: public repositories only.** The shared runner has one cache and one credential store, so it
gives no isolation between tenants or users. Private repositories arrive together with per-job containers.

**GitHub lookup MCP tool (server-side, read-only).** The runner has no GitHub token, so related-work
discovery goes through a tool the server exposes to the agent over MCP: PRs, issues and search, limited to
the PR's repository and its related repositories. The server executes calls with the requesting user's
token. The tool cannot write. Related work is found entirely by the LLM through this tool.

**Agent context and rules**
- The agent uses the `claudius` plugin directly (severity and system-rating rules, review skills) instead of
  copies of those rules in the prompt.
- Review lenses in the prompt: reuse of existing code, right layer / ownership, data design, behaviour at
  boundaries, performance and abuse.
- **Project profile**: risk is rated at system scale against a project profile — MemCan memory with
  `type: project-profile`, or `.claudius/project-profile.md` on the base ref. Every rating opens with what a
  production user or operator would notice. Without a profile, ratings are component-level and the PR
  context says so.
- **Calibration memory**: feedback is stored locally and written to MemCan; `overrated` / `underrated` are
  saved as severity calibration examples that later analyses read through the MemCan MCP.
- Tests are not shown or walked through, but the agent may read them as evidence ("covered by test X").
- Deterministic, free signals from Not analysed material (failing CI tests, lines removed from tests,
  changed dependencies) are surfaced by the server; the LLM does not interpret that material in v1.

**Job execution interface (designed for per-job containers)**

Target model (later version): the server reads the tenant's config, starts one container per job with the
tenant's tokens, the container checks out the repository (or unpacks its cache), runs the agent and exits.
v1 must reach that model by adding one new implementation, without changing the pipeline or the runner logic:

1. `JobSpec` is self-contained: job id, kind, repo clone URL, base and head SHA, prompt, output JSON schema,
   limits, LLM provider and model, MCP endpoints, optional credentials (repo token, LLM credential reference).
   Nothing is passed through shared volumes or ambient container state. In v1 the repo token is absent.
2. `JobExecutor` (server-side trait): `submit(JobSpec) → JobHandle`, `events(JobHandle) → stream`,
   `cancel(JobHandle)`. The pipeline only talks to this trait. v1 has one implementation, `QueueExecutor`
   (puts the job on the DB queue for the long-lived runner). A later `ContainerExecutor` starts a container
   per job. The server never assumes that a runner is long-lived or that two jobs share a filesystem.
3. Runner core is one function: `execute(JobSpec, EventSink) → JobResult` — fetch, checkout, run agent,
   clean up. The v1 polling loop is a thin wrapper around it; a one-shot entry point for per-job containers
   is a second thin wrapper, not built in v1.
4. Runner ↔ server protocol is the same in both models: authenticated HTTP — claim job, post events,
   post result. Each job carries its own short-lived token scoped to that job.
5. Caches are an optimisation only: a job must succeed with an empty cache.
6. Per-job containers must not carry the stack's `com.docker.compose.project` label (Portainer attributes
   containers to stacks by it). Docker socket access goes through a restricting proxy. Both are later work.

**Pipeline** (each step is a resumable job; partial results are persisted)
1. Ingest: PR metadata, threads, checks; fetch base and head into the server's bare mirror.
2. Pre-pass (deterministic, language-agnostic): hunks, classification through tech plugins (path globs and
   line regexes), moved-block candidates (`git diff --color-moved` + token-normalised hashes),
   whitespace-only changes, counters (lines removed from tests, changed dependencies).
3. Plan: the agent partitions hunks into flows (groups). Huge PRs: per-directory summaries first, then
   grouping.
4. PR context (own job): problem and why, related work through the GitHub lookup tool, fit questions with
   evidence, project profile lookup. Independent of step 5; queued like any job.
5. Analyse: one agent run per group — flow graph (structured), fragments with called by / decides / next /
   why, annotations, call hierarchy, assessment, out-of-diff impact, pattern repetitions.
6. Verify: schema, ranges within files, coverage of every changed line (uncovered lines become
   "Unexplained"), move claims, flow-graph consistency (edges reference existing nodes, every node maps to a
   fragment); up to 2 repair runs.

Ask is a further job on the same queue. There is no summarise step: no PR-level summary or overall risk.

**Tech plugin** (trait): `detect(file)`, `classify(file) → ranges of test / comment / doc / generated`;
later `symbols`, `call_hierarchy`. v1 ships `generic`, with built-in "not analysed" rules of two kinds:
- path globs: `tests/`, `*_test.go`, `*.md`, lockfiles, `// Code generated` headers;
- line regexes: for example lines matching `^\s*//` (comment-only lines).
Custom rules come later. Rust and Go plugins come in v2.

**LLM provider** (trait): `run(prompt, cwd, json_schema, limits) → event stream → {structured, session_id,
usage}`, `resume`, `cancel`. Credential types: `codex_chatgpt` (subscription; owner's tenant only) and
`api_key` (required for other tenants: Anthropic forbids subscription tokens in multi-user services, OpenAI
is unclear). See `/data/artifacts/smart-review/2026-11-16/research/llm-runtime.md`.

**Comments** — the server writes to the user's GitHub pending review with the user's token: the first
comment creates the pending review, later ones are added, an existing pending review is reused; edit and
delete act on GitHub; the user submits on GitHub. Expected API: GraphQL `addPullRequestReview` (pending) and
`addPullRequestReviewThread`. A comment on a line without annotation is also saved as feedback
`missed by the LLM`.

**Multi-tenancy** — every table carries `tenant_id`; `tenants`, `users`, `memberships` exist from v1 with
one seeded tenant `default`. No tenant UI.

**Security** — PR content is untrusted (prompt injection): read-only agent, no repo-provided agent config,
egress allowlist, only the LLM credential in the runner. The GitHub lookup tool is read-only and limited to the repository and its related repositories. Secrets encrypted at rest. Cookie sessions, CSRF, secure-cookie and trusted-proxy settings so the app
can sit behind a TLS reverse proxy.

## 6. Technology

Chosen to match the owner's active projects (penny, gamiscreen, memcan). sqlx (penny) and diesel
(gamiscreen) are tied there; sqlx wins as async-native with axum.

| Concern | Choice |
|---|---|
| Server | Rust, tokio, axum 0.8, tower-http, tracing, thiserror, clap, dotenvy |
| Database | sqlx with SQLite only. All database-dependent code (queries, migrations, row types) lives in one module behind a repository interface; nothing else touches sqlx. Moving to PostgreSQL later is a rewrite of that module, not a supported switch. |
| API types | serde; `schemars` (JSON schema for agent output), `jsonschema` (validation), `ts-rs` (TypeScript types) |
| GitHub | GitHub App with user access tokens; `octocrab` or `reqwest`; GraphQL for the pending review |
| Secrets | `aes-gcm` |
| MemCan | `rmcp` client (feedback writes); the agent uses MemCan MCP directly |
| Agent tools | `rmcp` server side for the read-only GitHub lookup tool |
| Diff / moves | `git` subprocess, `similar` |
| Frontend | React 19, TypeScript, Vite, TanStack Query, react-router, Tailwind 4; code shown by an own line renderer (plain HTML rows) with Shiki for syntax colouring, no editor component; flow diagram rendered by the UI from the graph data; vitest, Playwright; embedded with `rust-embed` |
| LLM | Codex CLI (`codex exec`) as a subprocess; no SDK, no third-party crates |
| Packaging | docker compose: `server`, `runner`, `egress-proxy`; named volumes |

Exact versions are pinned at implementation time from the registries.

## 7. Large files

Measured 2026-11-16 on dashpay/platform, tenderdash and dash-evo-tool: large files are routine. In
dash-evo-tool 45% of recent changes touch a file over 1000 lines (8% over 5000); platform has hand-written
files of 14–16k lines and generated gRPC clients of 135–140k lines.

Rules:
1. The UI never renders a whole file. The workspace and tour show fragments.
2. Shiki colours only the displayed lines. Occasional wrong colours (a fragment starting inside a
   multi-line comment or string) are accepted.
3. Generated files are classified as Not analysed by path rule before any agent run; they are never rendered
   and never given to the agent.
4. The agent reads line ranges and searches; prompts tell it not to load large files whole.
5. The API serves file content by line range.

## 8. Open points
- Codex plan tier vs usage windows for very large PRs.
- GraphQL pending-review API (`addPullRequestReview`, `addPullRequestReviewThread`): verify behaviour, reuse
  of an existing pending review, and edit/delete support at implementation.
- Codex's own sandbox (bubblewrap/landlock) inside an unprivileged container: verify it works or must be
  disabled in favour of the container sandbox.
- Rust and Go plugins in v1 instead of v2 (test regions inside production files need parsing).
- Portainer handling of containers created outside the stack (the later per-job model).
- Precedence when both a MemCan `project-profile` and `.claudius/project-profile.md` exist, and who creates
  the profile.
- Where per-repository "related repositories" are configured (Settings shows them read-only).
- How the `claudius` plugin is made available to the Codex agent in the runner image.
- Handling of `refactor_pure` and `rename` groups: they are analysed as ordinary groups unless the text of a
  moved block is identical (quiet row); whether a deterministic rename check earns its own badge is undecided.
