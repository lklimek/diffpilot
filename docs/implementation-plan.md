# smart-review — implementation plan (v1)

Status: draft for owner approval, 2026-11-16. Authoritative design: `docs/design-brief.md` (cited as "brief §N").
This plan says HOW v1 is built and by whom; it does not change the design. The owner's decisions on the
questions raised while planning are in §16 and are reflected in the brief.

## 1. Start here (coordinator, fresh session)

You coordinate and guard the architecture. You do not write application code; every code change is made by a
delegated agent in its own worktree. You talk to the owner in Polish; everything in the repo or on GitHub is
English.

**Read, in this order**
1. `CLAUDE.md`, then this file §1–§5 completely, then §11 (traceability) and §16 (owner decisions).
2. `docs/design-brief.md` completely. Mockups: `docs/mockups/index.html`; `05-tour.html` + `mockup.js` hold the
   example PR `acme/node-client#482` used by every fixture.
3. `docs/decisions/` (ADRs), `docs/spikes/` (spike findings), `docs/contracts/` — once they exist.
4. Background outside the repo, only when a work package needs it:
   `/data/artifacts/smart-review/2026-11-16/research/llm-runtime.md`,
   `/data/artifacts/smart-review/2026-11-16/research/prior-art.md` (§5 candidate JSON — see the drop list in §7),
   `/data/artifacts/smart-review/2026-11-16/reviews/psych-fable.md`, `…/reviews/psych-astra.md`.

**Load skills** before the first dispatch: `claudius:grand-admiral`, `claudius:delegate`,
`claudius:git-and-github`, `claudius:coding-best-practices`; `claudius:architecture` for contract reviews and
change requests; `claudius:review-changes` at every merge gate.

**Tracking** — one record: `/data/artifacts/smart-review/implementation-tracking.md` (outside git, survives
reboot). One row per work package (WP): `WP | status (todo/dispatched/review/merged/blocked) | branch |
worktree | agent id | base SHA | review report | notes`. Update before and after every dispatch, review and
merge. MemCan (`project="smart-review"`) holds only durable decisions (memories) and one TODO per open owner
question (§16). Review reports and spike evidence: copy to `/data/artifacts/smart-review/YYYY-MM-DD/` right after
each round.

**Resume after an interruption**
1. Re-read the tracking file; `list_todos(project="smart-review", status="pending")`;
   `search(query="smart-review implementation", project="smart-review")`.
2. Reconcile with reality: `git branch -a`, `git worktree list`, `gh pr list`, `git status` in every worktree
   under `/data/git-worktrees/home-ubuntu-git-smart-review-*`. Git state wins over the file.
3. Worktree with uncommitted work and no live agent: dispatch a fresh agent into the same worktree with the
   original brief plus "first review the existing uncommitted diff, then continue". Never delete it.
4. Continue with the earliest stage whose exit gate is not met.

**First session only**
1. Commit `CLAUDE.md` and this plan on `docs/initial-design` (the local repo has no remote; `main` holds one
   empty commit).
2. Ask the owner to create `github.com/lklimek/smart-review` (or to authorise creating it) and to push or let
   you push `main`; the base branch on GitHub needs the owner. Then push `docs/initial-design` and open the
   design PR. The repository is public with GitHub Actions enabled.
3. Collect the owner prerequisites (§6.1) before dispatching stage 0.

**Git flow**
- Stage branch `feat/s<N>-<name>`; WP branches cut from it at a recorded SHA, each in a pre-created worktree
  `/data/git-worktrees/home-ubuntu-git-smart-review-<wp>`. The coordinator merges WP branches into the stage
  branch (merge, no rebase, no amend, no force-push) and pushes feature branches itself (`ghsudo git push`).
- One PR per stage (stage 3: one PR per merge point M3a/M3b/M3c). Target ≤ ~3,000 changed lines excluding
  generated files and fixtures; split larger PRs at merge points. The owner merges PRs into `main`.
- Gates: each stage is one PR (stage 3: three) merged by the owner. Stage 2 starts only after the owner has
  approved the contracts (§7). The next stage may start stacked: stage N+1 is cut from the tip of stage N
  while stage N's PR awaits the owner's merge; never two unmerged stacked stages.
  Review fixes requested by the owner on stage N are merged forward into N+1.

## 2. Standing brief block (paste into EVERY coding brief)

```
Host and conventions (mandatory):
- Rust: thiserror typed enums for all errors (no anyhow/eyre/Box<dyn Error>/String errors); no unwrap/expect
  outside tests; tracing for logs; clippy -D warnings; #[expect(.., reason)] never #[allow]; TDD (tests first,
  failing first); one-line /// doc on every public item; verify each new dependency's latest version on the
  registry when adding it; prefer crates already used by penny/gamiscreen/memcan.
- Heavy jobs (host: 16 CPUs, ~15 GB RAM): at most ONE heavy job at a time across the whole session.
  * cargo test/clippy/nextest ONLY through the claudius cargo-cached.sh wrapper (absolute path from the
    SessionStart context; currently /home/ubuntu/.claude/plugins/cache/lklimek/claudius/12.0.0/scripts/cargo-cached.sh),
    narrowest -p, one command per outcome, foreground with a timeout; export CLAUDIUS_MAX_PARALLEL_BUILDS=1.
    Never set CARGO_TARGET_DIR, never raise `[build] jobs = 2` or NEXTEST_TEST_THREADS=4, never cargo check.
    A wait for the build slot may exceed your tool timeout: re-run the same command; never bypass the wrapper.
  * Docker image builds are heavy jobs: only when the coordinator has told you the slot is yours.
  * Web: vitest --maxWorkers=2, Playwright workers=2, one Playwright run at a time.
- Report the ledger line (command, tree key, exit, log path) and confirm the log names your new tests.
- Git: work only in your worktree <ABS_PATH>; first action `cd <ABS_PATH> && git merge --ff-only <SHA>`;
  re-check `pwd` before every git write. Conventional commits. Commit, never push, never amend, never force.
  Touch only your "Owned paths" plus the shared files of plan §4.3 under its rules. Contract, migration,
  route, trait or fixture-shape changes: stop and report a change request.
- Storage: scratch in /data/tmp, never large files in /tmp. Language: code, comments, commits in English.
- Keep it simple: no abstraction beyond the seams in plan §4.
- You are a subagent; report with SendMessage(to: "<coordinator name>"): approach, tests added, ledger line,
  self-review passes, out-of-scope findings, skills used.
```

## 3. Repository layout and dependency direction

```
Cargo.toml          workspace (resolver 3, edition 2024), [workspace.dependencies] for every crate of §5, [workspace.lints]
crates/contracts/   wire types: agent output, HTTP API DTOs, SSE events, errors, JobSpec + runner protocol;
                    schema and TS generation. Pure data, no IO.
crates/analysis/    deterministic, language-agnostic: hunks, TechPlugin + `generic`, moved blocks, counters,
                    related repos, verification, coverage, tour ordering. Runs `git` read-only on a given path.
crates/db/          THE database module: sqlx/SQLite, migrations, repository API (one file per aggregate).
crates/server/      axum app: auth, sessions, secrets, GitHub (REST/GraphQL, bare mirror), pipeline, prompts,
                    Ask, JobExecutor + QueueExecutor, runner protocol, SSE, MCP lookup tool, MemCan client,
                    SPA embedding (feature `embed-ui`, off in dev/test builds).
crates/runner/      runner: execute(JobSpec, EventSink) -> JobResult, polling loop, LlmProvider + Codex provider
                    (+ fake provider behind feature `fake-provider`), claudius checkout refresh, mirror cache.
tools/fake-github/  dev/test only: fake GitHub REST/GraphQL/OAuth serving recorded fixtures. Never in images.
web/                React SPA: src/api/generated/ (checked in), src/api/client.ts (only fetch layer),
                    src/components/<block>/, src/screens/<screen>/, src/mocks/ (msw), tests/ (Playwright vs msw).
e2e/                full-stack Playwright (server + runner + fake GitHub + fake MemCan).
prompts/            agent prompt templates per job kind.     schemas/agent/   generated agent-output schemas.
fixtures/pr-482/    example PR from the mockups (§7 WP1-B).  deploy/          Dockerfiles, compose, proxy config.
docs/               brief, plan, contracts/, decisions/, spikes/, runbook.
```

```
            contracts  <──────────────┬───────────────┬──────────────┐
                ^                     │               │              │
   analysis ────┘          db ────────┘     runner ───┘   tools/fake-github (dev only)
       ^                    ^
       └──── server ────────┘   (server -> contracts, analysis, db)
   web -> web/src/api/generated (produced from contracts); never imports server code
```

Nothing depends on `server` or `runner`. `runner` never depends on `db`, `analysis` or GitHub crates. `sqlx`
only in `crates/db/Cargo.toml`; `rmcp` server features only in `server`; GitHub client code only in
`server::github`. Enforced by `scripts/check-architecture.sh` (reads `cargo metadata`) in CI from stage 1.

## 4. Guarding the architecture

### 4.1 Rules
"Guarding" = the coordinator sends back any merge that breaks a rule, however good the code is otherwise.

| # | Rule (brief ref) | Check |
|---|---|---|
| A1 | Dependency direction of §3 | `check-architecture.sh` in CI |
| A2 | All DB code in `crates/db`; its public API exposes no sqlx types (brief §6) | A1 script + review |
| A3 | Pipeline and Ask talk only to `JobExecutor` (`submit`, `events`, `cancel`); `QueueExecutor` is the only v1 impl; no server code assumes a long-lived runner or a shared filesystem (brief §5 items 1–5) | review; grep server for runner paths/volumes |
| A4 | `JobSpec` self-contained; runner core is `execute(JobSpec, EventSink) -> JobResult`; loop is a thin wrapper; a job succeeds with an empty cache | runner test with empty cache |
| A5 | Only `runner::provider` knows Codex flags and events; the rest sees normalised events and typed errors | review |
| A6 | Only `analysis::tech` knows path globs / line regexes; `TechPlugin` = `detect`, `classify`, `call_hierarchy -> Option<..>`; `generic` returns `None` | unit test |
| A7 | Every UI block is a self-contained component: props in, callbacks out; no fetching, router hooks or screen imports in `web/src/components/**`; screens only arrange (brief §5 Frontend structure) | ESLint `no-restricted-imports` |
| A8 | Contract types only from `crates/contracts`; no hand-written TS duplicates; generated files current | drift test (§5) |
| A9 | Trust and wording rules (brief §1, §4.3) | tests of §12.4 |
| A10 | Security design of §5.4: runner holds the LLM credential and its runner secret only; agent process separated from them; MCP token ≠ job token; MCP tool read-only and scoped; secrets encrypted at rest, never logged, never sent to the browser | WP3-RUN/WP3-OPS tests, security reviews |
| A11 | Rust conventions of §2 | `[workspace.lints]` (`unwrap_used`, `expect_used`, `allow_attributes` deny), `clippy.toml` `allow-unwrap-in-tests`, CI grep for anyhow/eyre |
| A12 | Simplicity: no trait, crate, service or config option the brief does not name, unless an ADR says why | review |

### 4.2 Per-merge checklist (WP branch → stage branch)
1. Diff touches only owned paths and §4.3 shared files under their rule (`git diff --stat`).
2. Report has the ledger line for the current tree and names the new tests; each acceptance item maps to a test.
3. A1–A12 hold — read the diff, not the summary.
4. Independent review (`claudius:review-changes`): basic mode for ≥50 code lines; full mode (with security
   pass) for WP2-AUTH, WP3-AUTH, WP3-REVIEW, WP3-MCP, WP3-RUN, WP3-OPS. Blocking findings go back to the same
   agent; the reviewer re-checks. Report archived.
5. Generated artefacts current (drift test in the ledger).
6. After the merge: one targeted test run of the touched crates/packages on the merged tree; a review of the
   stage branch covers only the merge resolutions. Update tracking.

### 4.3 Shared files (collision points)
| File | Rule |
|---|---|
| `Cargo.toml` (workspace deps) | All §5 crates pre-declared in WP1-S; a WP adding a crate adds one line. |
| `Cargo.lock`, `web/package-lock.json` | Agents commit their lock changes; on conflict the coordinator takes the stage branch's version and lets `cargo metadata` / `npm install` re-resolve minimally (no `cargo update`). |
| `crates/*/Cargo.toml` | Add-only lines for the WP's own deps. |
| `crates/server/src/lib.rs`, `crates/db/src/lib.rs`, `crates/analysis/src/lib.rs` | Module lines pre-created in WP1-S; agents do not edit them. |
| `crates/server/src/state.rs` (`AppState`), `crates/server/src/config.rs` | All fields pre-declared in WP1-S from §5; a WP fills only its own constructor/validation; new fields = change request. |
| `crates/server/src/api/mod.rs` | One `.merge(<module>::routes())` per module, pre-created in WP1-S. |
| `web/package.json`, `web/src/app/routes.tsx` | Add-only lines. |
| `web/src/mocks/handlers/<endpoint>.ts` | One file per endpoint, owned by the WP that owns the endpoint's UI consumer. |
| `fixtures/pr-482/**` | Shape changes are change requests; content changes belong to the WP whose producer changes (§7). |

**Change request (CCR).** Contracts, migrations, routes, trait signatures, `AppState`/`Config` fields and
fixture shapes are frozen after stage 1. An agent needing a change stops and reports. The coordinator decides
(architecture skill), applies it in one small commit on the stage branch (or a short WP), regenerates
artefacts, writes an ADR if not trivial, and tells running agents to merge the new SHA.

## 5. Contracts

### 5.1 Source of truth and drift prevention
| Contract | Source of truth | Generated (checked in) | Drift guard |
|---|---|---|---|
| Agent output per job kind (`plan`, `pr_context`, `analyse_flow`, `ask`) | `crates/contracts/src/agent/` (serde + `schemars`) | `schemas/agent/<kind>.schema.json`, Codex-strict subset (S1a, S7) | test `generated_artifacts_are_current` regenerates and compares; `UPDATE_GENERATED=1` rewrites; CI also `git diff --exit-code` |
| HTTP API DTOs, error model, SSE events | `crates/contracts/src/api/` (serde + `ts-rs`, gamiscreen's `ts` feature pattern) | `web/src/api/generated/*.ts` | same test |
| Routes | `crates/contracts/src/api/routes.rs` (path constants + method) | `web/src/api/generated/routes.ts` | same test; server router built from the constants; vitest: every route has an msw handler |
| Runner protocol, `JobSpec`, `JobEvent`, `JobResult`, state machines | `crates/contracts/src/job/` | — | Rust tests |
| Database | `crates/db/migrations/*.sql`, repository API `crates/db/src/repo/*.rs` | `docs/contracts/db-schema.md` (summary) | migrate-empty-DB test; CCR |
| Seams | `JobExecutor` (`server::jobs`), `LlmProvider` (`runner::provider`), `TechPlugin` + verify/coverage/ordering/moves/related-repo signatures (`analysis`) | — | review (A3–A6) |
| Fixtures | `fixtures/pr-482/` | API golden generated by the server from M2 | §7 WP1-B tests |

Versions checked on the registries 2026-11-16 (re-verify when adding): `schemars` 1.2.3, `ts-rs` 12.0.1,
`jsonschema` 0.61.2, `axum` 0.8.9, `sqlx` 0.9.0 (penny: 0.8), `rmcp` 3.10.0, `octocrab` 0.54.9, `aes-gcm` 0.11.1,
`rust-embed` 8.14.0, `similar` 3.2.1, `wiremock` 0.6.5, `proptest` 1.12.0, `insta` 1.49.0; npm `react` 19.3.1,
`vite` 8.3.8, `@tanstack/react-query` 5.107.1, `react-router` 8.5.1, `tailwindcss` 4.3.5, `shiki` 4.6.0,
`msw` 3.0.6, `vitest` 5.1.0, `@playwright/test` 1.65.1, `typescript` 7.0.2 (if tooling breaks: penny's 5.9 line,
recorded in an ADR).

### 5.2 Decisions fixed in stage 1 (ADR-001, WP1-S/WP1-A)
sqlx runtime queries (`query`/`query_as`, as penny; no macros, no offline data); npm with `package-lock.json`
(as penny and gamiscreen); AES-GCM key from env `SR_SECRET_KEY` (32 bytes, base64, created by the runbook; no
rotation in v1); server port 3000 and `SR_PUBLIC_URL` (callback base); configurable GitHub API, OAuth and clone
base URLs (dev: fake GitHub and a `file://` or `git daemon` remote); model selection is one function
`select_model(job kind, context) -> ModelChoice { model, reasoning_effort }` in `server/src/model.rs`, a
placeholder returning Sol at high effort for every kind (exact model id from S1a); no per-kind config.

### 5.3 Design rules for the contracts (binding for WP1-A)
- Closed enums exactly as brief §3 (classification, fragment kind, annotation type, feedback values, node kind,
  provenance badges `text_identical` / `classified_by_path` / `classified_by_line_rule`).
- No `confidence`, score, percentage or float anywhere; no PR-level risk. A schema test asserts that the only
  numbers are integer line numbers and counters.
- Every annotation and observation carries `basis: fact | inference`; `fact` requires ≥1 evidence reference
  (`file`, `line`); verify rejects a fact without evidence (brief §1 "observed facts carry a line reference").
- Lines are 1-based inclusive at the head SHA; ids are strings stable within one analysis.
- `JobSpec` carries `ModelChoice` and no session reference; `LlmProvider` is `run` + `cancel` (no `resume` in
  v1). An `ask` job carries a context pack: step data, the flow's analysis, earlier questions and answers of the
  step.
- Agent schemas use the Codex structured-output subset found in S1a/S7; the server re-validates with
  `jsonschema`.
- Call hierarchy is in no agent schema. API `Step.call_hierarchy: Option<CallHierarchy>` is filled by the
  server from `TechPlugin::call_hierarchy`; `None` in v1 (UI: "n/a — needs a language plugin").
- Deterministic facts (gutter marks, badges, counters, "N lines differ", changed-line sets) are server-set API
  fields in types separate from LLM content.
- Storage: validated agent output as JSON documents (PR context; one per flow); relational tables only for
  queue and mutable state: tenants, users, memberships, sessions, encrypted tokens, repos, pull requests,
  analyses, jobs (+ bounded event log), threads/checks snapshot, review state, questions, feedback, comment
  idempotency keys, settings, runner status. Every table has `tenant_id`; tenant `default` seeded (brief §5).
- Error model: `ApiError { code: ApiErrorCode, message }`, `message` = plain user-facing text, never internal
  error text; status mapping in `docs/contracts/http-api.md`.

**HTTP API sketch** (finalised in `docs/contracts/http-api.md` with an endpoint × fixture-state inventory):
`/auth/github/login|callback|logout`, `GET /api/v1/me`; `POST /api/v1/pr/resolve` (URL → PR, existing
analysis, head moved); `POST /api/v1/analyses`; `GET …/analyses/{id}`; `GET …/{id}/events` (SSE);
`POST …/{id}/cancel|retry`; `GET …/{id}/tour`; `GET …/{id}/lines?path&from&to` (range-capped, brief §7);
`GET …/{id}/diff?path&from&to`; `GET|PUT …/{id}/review-state`; `POST …/{id}/steps/{step}/questions`;
`POST …/{id}/feedback`; `GET …/{id}/pending-review`, `POST|PATCH|DELETE …/pending-review/comments` (idempotency
key); `GET /api/v1/settings`, `PUT …/settings/language`, `POST …/settings/llm-check`;
`GET /api/v1/repos/{owner}/{name}/settings`. Mutating routes require the CSRF header.

**Runner protocol sketch**: `POST /runner/v1/claim` (runner secret) → `JobSpec` + job token + MCP token;
`POST /runner/v1/jobs/{id}/events` (job token; response carries `cancel_requested`); `POST …/{id}/result` (job
token, idempotent per attempt); `POST /runner/v1/heartbeat` (response may request a credential check; the
runner runs a cheap check and reports "working" / "not working"). MCP lookup tool at `/mcp`
(streamable HTTP), bearer = MCP token. Job states `queued → claimed → running → succeeded | failed | cancelled |
paused{resumes_at}`; expired lease → `queued`, attempt + 1, bounded.

### 5.4 Security design (WP1-T, reviewed with the contracts; audited again in stage 4)
| # | Requirement | Tested in |
|---|---|---|
| T1 | Two uids in the runner image: `runner` owns runner secret, job token, mirror cache, claudius checkout, master `CODEX_HOME`; Codex and every agent command run as `agent` with a fresh work dir, a per-job `CODEX_HOME` (generated config + copy of `auth.json`), read-only skills. `runner` copies `auth.json` back after the job if it changed (one job per credential, so no race). | WP3-RUN, WP3-OPS |
| T2 | Agent environment scrubbed: only what Codex needs (home, PATH, proxy vars, MCP bearer variable). | WP3-RUN |
| T3 | MCP token separate from the job token: read-only lookup tool, scope = PR repo + related repos, executes with the requesting user's token server-side, expires at job end. The job token never reaches the agent. | WP3-MCP, WP3-RUN |
| T4 | Container: non-root, `cap_drop: ALL`, `no-new-privileges`, read-only root fs except work dir and per-job `CODEX_HOME`, no Docker socket, egress only through the allowlist proxy. If S1b disables Codex's own sandbox, T1–T4 are the compensating controls (ADR, owner informed). | WP3-OPS |
| T5 | Repository-provided agent config (AGENTS.md, `.codex/`, hooks, MCP config) never loaded (S1b mechanism). | WP3-RUN |
| T6 | MemCan from the agent: full MemCan MCP (read and write). **Accepted risk** (owner, 2026-11-16): an injected PR may write misleading memories; distinguishing agent-written memories is future MemCan work. Feedback and calibration examples are written by the server. | WP3-RUN, WP3-MCP |
| T7 | Server: cookie session (Secure, HttpOnly, SameSite), CSRF, trusted-proxy setting, tokens encrypted with AES-GCM, nothing secret in logs, errors or the browser. | WP2-AUTH, WP3-AUTH |

## 6. Stage 0 — spikes (time-boxed, throwaway code)

Spike code in `/data/tmp/smart-review-spikes/<id>/`; at the end its reusable parts (Dockerfile, compose snippet,
recorded JSONL, sanitised GitHub responses, scripts) are archived with the evidence in
`/data/artifacts/smart-review/YYYY-MM-DD/spikes/<id>/`. Each spike writes `docs/spikes/S<id>-<name>.md`
(≤150 lines: question, runs, evidence, finding, decision, plan impact) and an ADR where it decides something.
Time box: one agent session (~3 h); overrun → stop and report what is known.

### 6.1 Owner prerequisites (ask in one message, Polish)
O1 GitHub repo (§1). O2 credentials and callback configuration of the owner's existing GitHub App (user access
tokens; pull requests read/write, contents read, checks read, issues read, metadata read; callback
`SR_PUBLIC_URL/auth/github/callback` on the LAN host); the owner installs it on the target repositories (owner
has dashpay org admin rights).
O3 a scratch public repo with an open PR (S3, M3b smoke). O4 device login: when S1a's container is ready the
agent pauses and the coordinator asks the owner to run `codex login --device-auth` inside it, writing to the
spike `CODEX_HOME` volume. **Never mount or copy the host `~/.codex`** (token rotation would break the owner's
own login). O5 MemCan URL and whether the runner needs a token (§16 open item).
O6 one real medium public PR for S7 (a merged 500–1,500-line PR of an owner repo). Quota: no cap on the Pro
plan; keep every real run purposeful.

### 6.2 Spikes
| Id | Question | Done when | If it fails |
|---|---|---|---|
| S1a Codex basics | Unprivileged non-root container with subscription auth on a volume (pause for O4); `codex exec --json --output-schema`: event stream shape, accepted schema subset (required/nullable, `additionalProperties`, enums, depth, `oneOf`), token refresh written back; exact model id for "Sol" and the reasoning-effort setting; hosts contacted | finding + 3 recorded JSONL runs (success, schema error, cancelled) + host list | Headless subscription auth impossible → stop; owner decides on an API key. |
| S1b Codex isolation | Cancel via SIGTERM; Codex sandbox (bwrap/landlock) inside the container; ignoring repository config (`--ignore-user-config`, `--ignore-rules`, AGENTS.md handling); MCP client config (streamable HTTP, bearer env var, `enabled_tools`) in a per-job `CODEX_HOME`; usage-limit error shape (docs/source if not triggerable) | finding doc | Sandbox fails → off, T1–T4 compensate. Repo config not ignorable → runner removes those files from the work dir before the run (ADR). |
| S2 claudius skills | How Codex 0.167 discovers skills from a git checkout of `lklimek/claudius` at a configurable ref; is `severity` actually used? | finding + run log | Inject needed skill files from the checkout into the prompt at job start. |
| S3 Pending review | GraphQL on O3: create pending review with first thread, add thread, multi-line, deleted-line (LEFT), edit, delete, find + reuse existing pending review, file-level comment with a link to a line outside the diff, list pending comments; with a GitHub App user token (from S4) | finding + **sanitised recorded responses** (become fake-github fixtures) | REST pending-review endpoints; if file-level comments cannot be pending, post them as the review body (ADR). |
| S4 App user tokens | With the owner's existing App: web flow with LAN callback; expiry and refresh-token rotation; read/write on public repos with/without the App installed; installation status query | finding + sanitised recorded responses | Device flow login (ADR). |
| S5 MCP from runner | Minimal `rmcp` streamable-HTTP tool in axum with a bearer token, called by Codex in the S1a container through the proxy; `enabled_tools`; MemCan MCP reachable the same way | tool call visible in Codex JSONL | stdio shim in the runner forwarding to a server endpoint with the MCP token (changes `JobSpec` — hence in the gate). |
| S6 Egress proxy | Runner on an internal network; proxy as only egress; allowlist = S1a hosts + github.com + MemCan + server; Codex, git and runner honour `HTTPS_PROXY`; denied host blocked and logged; proxy choice (maintenance, licence) | compose snippet + allowed/denied log | Codex ignores proxy env → owner decides: transparent proxy (NET_ADMIN on the proxy only) or DNS-level restriction (ADR). |
| S7 LLM capability | Draft `plan` and `analyse_flow` schemas in the S1a subset; run `plan`, then `analyse_flow` for 2–3 flows, on O6 with draft prompts: schema validity, coverage of changed lines, plausibility of flows and flow graph, runtime, usage, repairs needed | finding with numbers + outputs archived; go/no-go on the agent-output shape | Shape not producible → simplify the schema (fewer nested levels, split `analyse_flow` into graph + steps) before the freeze; record ADR; owner informed. |

Dispatch (`claudius:developer-bilby`, opus): agent 1 = S1a → S1b → S2; agent 2 = S3 + S4; agent 3 = S5 + S6
(after S1a's container exists); agent 4 = S7 (after S1a). S5 needs one cargo build: one-heavy-job rule.

**Exit gate stage 0**: S1a, S1b, S3, S4, S5, S7 decided (they shape contracts and `JobSpec`); S2 and S6 may
finish during stage 1 but before WP3-RUN / WP3-OPS start. Owner informed of every fallback taken.

## 7. Stage 1 — contracts and skeleton

| WP | Goal | Owned paths | Agent / model | Size |
|---|---|---|---|---|
| WP1-S Skeleton | Workspace, all crates with `lib.rs`/`main.rs` stubs, all §5 dependencies declared, lints, rustfmt, toolchain, server module skeleton (`state.rs`, `config.rs` with all fields, empty modules, router registration, feature `embed-ui`), `scripts/check-architecture.sh`, ADR-001 (§5.2) | root manifests and configs, `crates/*/Cargo.toml`, `crates/*/src/{lib,main}.rs`, `crates/server/src/{state,config}.rs`, `crates/server/src/api/mod.rs`, module stub files, `scripts/check-architecture.sh`, `docs/decisions/ADR-001-*` | developer-bilby, sonnet | ~400 |
| WP1-A Contracts | Everything in §5.1/§5.3 after WP1-S: contract types, generation + drift test, routes, traits, `analysis` public signatures (verify, coverage, ordering, moves, related repos), `db` repository API signatures (typed `NotImplemented` error, no `todo!()`), migration `0001_init.sql`, `docs/contracts/{http-api,sse-events,runner-protocol,db-schema,agent-output}.md`, `scripts/schema-smoke.sh` | `crates/contracts/**`, trait and signature files in `analysis`, `db`, `server/src/jobs/executor.rs`, `runner/src/provider/mod.rs`, `crates/db/migrations/**`, `schemas/**`, `web/src/api/generated/**`, `docs/contracts/**` | developer-bilby + architecture skill, opus, plan gate | ~1,000 (types) + docs |
| WP1-T Threat model | `docs/contracts/threat-model.md` (one page): trust boundaries, assets, T1–T7 refined with S1b/S5/S6 findings, compensating controls | `docs/contracts/threat-model.md` | security-engineer-smythe, opus | doc |

WP1-A inputs: brief §3, §5, §6; S1a/S1b/S3/S4/S5/S7 findings; mockup.js data shapes; §4–§5. From
`prior-art.md` §5 take the structure only and **drop**: `confidence`, `summary.overall_risk`,
`review_effort_minutes`, float `importance` (use an integer rank), `risk.likelihood/impact`, `noise` (→ Not
analysed, server-side), `bug_suspect` (→ `question`), `recommendation`, `tour_order`, `verified_by: llm_only`,
LLM `call_hierarchy`.

WP1-A acceptance: workspace builds; clippy clean and tests green for `-p contracts`; drift test green; schema
tests (closed enums, no floats, no confidence, `basis` + evidence rule); `check-architecture.sh` passes;
**schema smoke**: `scripts/schema-smoke.sh` runs one real `codex exec --output-schema` per job kind (S1a
container, O6 PR) and every final schema is accepted — log archived.

Coordinator gate: architecture review of contracts + threat model against brief and §4–§5; then **owner
review** (Polish summary + PR link). Nothing in stage 2 starts before the owner approves and merges.

After approval, in parallel:

| WP | Goal | Owned paths | Agent / model | Size |
|---|---|---|---|---|
| WP1-B Fixtures | Example PR `acme/node-client#482`, 5 flows / 14 steps (mockup.js data and its "5 flows, 14 steps" summary; the mockup DOM shows 12 blocks): `repo/{base,head}/` trees + helper building a git repo; `github/` from the S3/S4 recorded responses adapted to #482; `agent/plan.json`, `pr_context.json`, `flow-1..5.json`; `codex/*.jsonl` from S1a adapted; `api-seed/default/` only for the slice endpoints; `api-states/<state>.json` small JSON merge patches (RFC 7396) for `wrong`, `missed`, `noq`, `noprofile`, `resume` | `fixtures/pr-482/**`, `crates/contracts/tests/fixtures.rs` | developer-bilby, sonnet | data + ~200 |
| WP1-C CI | CI: fmt, clippy `-D warnings`, nextest, doctests, drift test, architecture check, wording check (web source strings only); web jobs no-op until `web/package.json` exists | `.github/workflows/ci.yml`, `scripts/check-wording.sh` | developer-bilby, sonnet | ~250 |

WP1-B acceptance: agent fixtures validate against `schemas/agent/*`; API seed and patched states deserialise
into contract types and re-serialise to the same JSON (round-trip catches unknown fields); fixture text passes
the wording rules. Fixture ownership afterwards: §4.3.

**Exit gate stage 1 (M1)**: contracts, threat model, fixtures and CI merged; schema smoke archived; CLAUDE.md
"Build and test" filled in.

## 8. Stage 2 — thin vertical slice

Goal: URL → analysis with fixture agent output → Tour renders flow 1 through real server, runner and browser.
Scope is deliberately thin: no pre-pass rules, no repair, no comments, no Ask; one happy path. Branch
`feat/s2-slice`. All WPs run in parallel; Rust WPs queue their cargo runs.

| WP | Goal | Owned paths | Depends | Agent / model | Size |
|---|---|---|---|---|---|
| WP2-JOBS | `QueueExecutor`, job queue (claim with lease, events, result, idempotent per attempt), job and MCP tokens, runner protocol endpoints, SSE progress | `server/src/jobs/**`, `server/src/api/{runner,events}.rs`, `db/src/repo/{jobs,analyses}.rs` | M1 | developer-bilby, opus | ~900 |
| WP2-AUTH | GitHub App web login against configurable base URL, session cookie, AES-GCM token storage | `server/src/{auth,secrets}/**`, `server/src/api/auth.rs`, `db/src/repo/{users,sessions,tokens}.rs` | M1 | developer-bilby, opus | ~700 |
| WP2-INGEST | PR resolve + ingest (REST metadata), bare mirror fetch from the configurable clone base, hunks (`analysis::diff`), pipeline skeleton (`pr_context` + `plan` → `analyse_flow` → schema validation → persist) via `JobExecutor`, tour and lines endpoints, `embed-ui` serving, `select_model` placeholder (Sol, high), fake-github contract test | `server/src/{github,pipeline,frontend}/**`, `server/src/model.rs`, `server/src/api/{pr,analyses,tour,lines}.rs`, `analysis/src/diff.rs`, `db/src/repo/{repos,pull_requests,documents}.rs`, `server/tests/**` | M1 | developer-bilby, opus | ~1,000 |
| WP2-RUN | Runner loop, protocol client, `execute()` (mirror cache fetch, fresh work dir checkout, provider run, event streaming, result, cleanup), fake provider (feature), **minimal Codex provider `run`** (exec + schema + JSONL parse; no resume/cancel/limits) | `crates/runner/**` | M1, S1a | developer-bilby, opus | ~900 |
| WP2-FAKE | `tools/fake-github`: OAuth, `/user`, PR/commits/files REST from `fixtures/pr-482/github` | `tools/fake-github/**` | M1 | developer-bilby, sonnet | ~400 |
| WP2-WEB | Vite + React + TS + Tailwind + TanStack Query + react-router; mockup design tokens and structure ported from `docs/mockups/mockup.css` into the Tailwind theme; `api/client.ts`; msw handlers from `api-seed`; ESLint rule A7; Tour minimal: left menu, PR context block, flow introduction, step block with plain code view (neutral gutter marks, one highlight), Previous/Next | `web/**` except `web/src/api/generated` | M1 | developer-bilby, sonnet | ~1,000 |

Acceptance (named tests):
- WP2-JOBS: claim/lease/expiry/re-queue; wrong or expired token rejected; result posted twice → stored once;
  SSE emits progress for a scripted job sequence.
- WP2-AUTH: login against wiremock GitHub; token row is ciphertext; session cookie flags; logout clears session.
- WP2-INGEST: `slice_pipeline` (server + scripted executor returning fixture agent output) → analysis
  `finished`; its tour JSON is written as the server golden `fixtures/pr-482/api/default/tour.json` with
  `UPDATE_GOLDEN=1` (first creation reviewed by the coordinator against `api-seed`, then `api-seed` is deleted);
  lines endpoint caps the range; fake-github contract test.
- WP2-RUN: fake provider reproduces fixture events/result; work dir removed on success/failure; empty cache
  works (A4); recorded S1a JSONL parsed by the Codex provider.
- WP2-WEB: vitest per block; Playwright (msw): PR context → flow 1 introduction → step 1 via `j`.

**M2 merge point**: merge all; dispatch **WP2-E2E** (developer-bilby, sonnet, ~250; owns `e2e/**`,
`deploy/dev/**`): full-stack Playwright — server (`embed-ui`) + runner (fake provider) + fake GitHub + fixture
remote: sign in, paste URL, progress reaches finished, Open review, flow 1 step 1 shows the fixture code. Then:
- **Real-Codex smoke (manual, coordinator + owner)**: the slice with the Codex provider on the O6 PR; analysis
  finishes or fails visibly; outputs archived; prompt or schema problems become stage-3 inputs.
- **Owner checkpoint 1**: Playwright screenshots of the slice beside the mockup, published under
  `/data/artifacts/smart-review/YYYY-MM-DD/screens/` (`http://agentic/smart-review/…`).
- Stage-2 PR.

## 9. Stage 3 — breadth

All WPs start after M2 unless "Depends" says otherwise. Run at most three Rust agents at once; UI and ops
agents fill the gaps. Every WP's acceptance items are named tests; trust tests per §12.4.

| WP | Goal (brief refs) | Owned paths | Depends | Agent / model | Size |
|---|---|---|---|---|---|
| WP3-AUTH | Refresh-token rotation, logout everywhere, CSRF, Secure/trusted-proxy settings, sign-in allowlist of GitHub logins (config; default the owner only), installation status (§2 Users, §5 Security) | `server/src/{auth,secrets}/**`, `server/src/api/auth.rs`, `db/src/repo/{users,sessions,tokens}.rs` | S4 | developer-bilby, opus | ~700 |
| WP3-REVIEW | Threads + checks ingest; pending review via GraphQL (create, reuse, add, edit, delete, count, list) per S3; comments API with idempotency key; a comment on a line outside the diff becomes a file-level comment whose body links to the line; feedback `missed_by_llm` when the comment is on a **changed line without annotation** (§3 Feedback, §5 Comments) | `server/src/github/{threads,checks,review}.rs`, `server/src/api/comments.rs`, `db/src/repo/{comments,threads}.rs` | S3 | developer-bilby, opus | ~900 |
| WP3-PREPASS | `generic` plugin (path globs, line regexes, generated headers; `call_hierarchy → None`), moved blocks (`--color-moved` + token-normalised hashes + `similar`), whitespace-only, counters, related repos from Cargo files, same org (§5 Pipeline 2, Tech plugin, §7) | `analysis/src/{tech,moves,counters,related}/**` | — | developer-bilby, opus | ~900 |
| WP3-VERIFY | Verification (schema, ranges, coverage → Unexplained, move claims, graph consistency, fact-needs-evidence, banned phrases in LLM text), repair feedback, tour ordering (behaviour by importance → LLM-classified refactors → LLM-classified tests → quiet rows; Unexplained first among quiet rows) (§1, §5 Pipeline 6, §4.3) | `analysis/src/{verify,coverage,order}/**` | — | developer-bilby, opus | ~800 |
| WP3-RUN | Codex provider complete per S1b/S2: per-job `CODEX_HOME`, repo-config neutralisation, normalised events, typed usage-limit error with `resumes_at`, `cancel`, claudius refresh (interval, ref), on-demand credential check, T1/T2/T3/T5/T6 | `crates/runner/**` | S1b, S2, WP1-T | developer-bilby, opus | ~900 |
| WP3-PIPE | Orchestration: pre-pass + verify integration, huge-PR plan (per-directory summaries then grouping), `pr_context`, one `analyse_flow` per flow, ≤2 repairs, progress `k of n`, pause/resume, cancel (partial results discarded), retry, failure before any flow (plain description + Retry), re-analyze on moved head, resume of an existing analysis, LLM status of threads located in a flow's steps from `analyse_flow` (others: none) (§5 Pipeline, §2 Resume, §4.2) | `server/src/pipeline/**`, `server/src/api/analyses.rs` | PREPASS, VERIFY merged first | developer-bilby, opus, plan gate | ~1,000 |
| WP3-PROMPTS | Prompt per job kind: review lenses, project profile union (file wins), LLM-text language, trust wording, "read ranges, never whole large files", generated files never given; no numeric likelihoods in LLM text; test code inside production files classified `test` (§5 Agent context, §3, §7) | `prompts/**`, `server/src/prompts.rs` | S7 | developer-bilby, opus | ~500 |
| WP3-ASK | Ask job on `JobExecutor`: fresh run with a context pack (step data, flow analysis, earlier Q&A of the step); questions API; answer streamed via SSE and stored as `Question`; `ask` claimed before the next batch job, no pre-emption (§5 Ask) | `server/src/ask/**`, `server/src/api/questions.rs`, `server/src/jobs/claim.rs`, `db/src/repo/questions.rs` | — | developer-bilby, opus | ~500 |
| WP3-MCP | GitHub lookup MCP tool (PRs, issues, search; read-only; T3 scope), MemCan rmcp client (feedback, calibration examples for overrated/underrated), feedback API (§5 lookup tool, Calibration memory) | `server/src/{mcp,memcan}/**`, `server/src/api/feedback.rs`, `db/src/repo/feedback.rs` | S5 | developer-bilby, opus | ~900 |
| WP3-STATE | Review-state API (ticks, last viewed block), settings API (language, `llm-check` → runner credential check, per-repo read-only rules, related repos, profile source), `diff` endpoint, heartbeat endpoint + runner status (§2 Progress, §4.4) | `server/src/api/{review_state,settings,diff,heartbeat}.rs`, `db/src/repo/{review_state,settings,runners}.rs` | — | developer-bilby, sonnet | ~700 |
| WP3-CODEVIEW | Line renderer: Shiki on displayed lines only; neutral mark on every changed line; LLM bar solid/dashed/dotted only on annotated ranges; `≠` moved; markers (remark, deleted site, thread, own comment); hover was → is only; comment-line highlight; inline old code (§4.3 Code view) | `web/src/components/code-view/**` | — | developer-bilby, opus | ~800 |
| WP3-DIAGRAM | Flow diagram from graph data (layout lib chosen and verified: `elkjs` 0.12.1 or `@dagrejs/dagre` 3.1.3); kind by colour AND border/marker; branch labels; current step; click → step (§4.3 Centre) | `web/src/components/flow-diagram/**` | — | developer-bilby, sonnet | ~500 |
| WP3-BLOCKS | Navigation list (labels for LLM-classified refactors and tests), PR context, flow introduction, step block (four facts + code slot), quiet-row blocks (moves, Not analysed, Unexplained, pending-review list), badge + tooltip primitives, help dialog (legend, shortcuts) (§4.3) | `web/src/components/{nav,pr-context,flow-intro,step,quiet,badge,tooltip,help}/**` | — | developer-bilby, sonnet | ~1,000 |
| WP3-INSPECTOR | Inspector (never empty; current block or selected annotation; Back to step) with 8 collapsible sections (state in localStorage; call hierarchy "n/a — needs a language plugin"); dock: Ask, feedback buttons, comment composer on any line (outside the diff: note "posted as a file comment") (§4.3 Inspector) | `web/src/components/{inspector,ask,feedback,comment}/**` | — | developer-bilby, sonnet | ~1,000 |
| WP3-SCREENS | Home (sign-in, URL, resume, head-moved banner, errors), Analysis progress (SSE; planning with indeterminate bar "Planning flows…", running, paused, failed incl. before any flow, finished; Home message "Analysis cancelled — analyze again"), Settings (§4.1, §4.2, §4.4) | `web/src/screens/{home,progress,settings}/**` | — | developer-bilby, sonnet | ~700 |
| WP3-TOUR | Tour assembly: top bar, 3 resizable/hideable/full-screen columns remembered, block sequence and flow order, keyboard map, ticks + "flows reviewed k / n", last viewed block, six fixture states (§4.3) | `web/src/screens/tour/**`, `web/src/app/**` | BLOCKS, INSPECTOR, CODEVIEW, DIAGRAM | developer-bilby, opus | ~900 |
| WP3-OPS | Dockerfiles (server: web build + Rust with `embed-ui`, `CARGO_BUILD_JOBS=2`; runner: pinned Codex CLI, git, ripgrep, two uids, no Docker socket), compose (server, runner, egress-proxy, named volumes, internal network, healthchecks, no runtime containers), `.env.example`, CI image builds and web jobs, release builds without `fake-provider`/fake GitHub (§5 Packaging, T1–T4) | `deploy/**`, `.github/**`, `.dockerignore`, `docs/runbook.md` | S6, WP1-T | developer-bilby, sonnet | ~700 |
| WP3-E2E | Fake MemCan; full-stack Playwright extended: comments (create/reuse/edit/delete), Ask, feedback, resume, head moved, the three hard cases, rendered-text wording in all states; compose-based run in CI | `e2e/**`, `tools/fake-memcan/**` | M3a + M3b | developer-bilby, sonnet | ~700 |

Key acceptance tests (in addition to the brief items in §11):
- WP3-AUTH: refresh rotation persisted; CSRF rejection; allowlist rejects a non-listed login; no token in logs
  (log-capture test).
- WP3-REVIEW: second comment reuses the pending review; double submit with one key → one comment (G-DOUBLE);
  edit/delete reach fake GitHub; `missed_by_llm` only for changed lines without annotation.
- WP3-PREPASS/VERIFY: proptest coverage invariant (every changed line in exactly one flow, Not analysed item or
  Unexplained); identical vs differing moves; generated file never in agent input; dangling edges, unmapped
  nodes, out-of-range lines and facts without evidence rejected; quiet rows never above a rated flow.
- WP3-RUN: golden JSONL incl. unknown events; usage limit → typed error; cancel kills and cleans; generated
  config has only the `JobSpec` MCP endpoints (lookup tool, MemCan); agent uid cannot read runner secret,
  master `CODEX_HOME` or mirror (container test); agent env contains no job token.
- WP3-PIPE: scripted-executor tests for happy path, repair success, repair exhausted → failed flow + Retry,
  usage limit → paused → resumed, cancel, failure before flows, head moved → new analysis without ticks,
  resume keeps ticks/answers/last block. WP3-PROMPTS: insta snapshots per job kind and language.
- WP3-MCP: no write tools; out-of-scope repo and expired/job token rejected; user token used (fake asserts).
- UI WPs: vitest per component (props in, callbacks out, no fetch); keyboard map; layout persistence.
- WP3-OPS: compose with fake provider passes full-stack Playwright; non-allowlisted host blocked; non-root.

**Merge points**
- **M3a (UI)**: BLOCKS, INSPECTOR, CODEVIEW, DIAGRAM, SCREENS, TOUR. Playwright (msw) for all six states.
  UX review by `claudius:ux-designer-diziet` (opus) of screenshots vs mockups and brief §4; **owner checkpoint 2**:
  six states beside the mockups, published as in M2. PR 3a.
- **M3b (backend)**: PREPASS and VERIFY before PIPE; AUTH, REVIEW, RUN, PROMPTS, ASK, MCP, STATE independent.
  **Manual real-GitHub smoke** on O3 with the real App: login, refresh, comment create/reuse/edit/delete. PR 3b.
- **M3c**: OPS and E2E. PR 3c.

## 10. Stage 4 — hardening and acceptance

| WP | Goal | Agent / model | Size |
|---|---|---|---|
| WP4-SEC | Audit of the merged tree against WP1-T: auth/session/CSRF, token storage, runner uids/egress, MCP scope, prompt-injection paths, secrets in logs, SSE auth, path traversal in lines/diff; fixes go to the owning WP's agent | security-engineer-smythe, opus | review |
| WP4-REAL | Codex on 3 public PRs chosen by the owner (small, medium, 10k+ lines with a large generated file): runtime, usage, validity, repair rate; prompt tuning in `prompts/` | developer-bilby, opus, owner present | ~300 |
| WP4-PERF | Synthetic large PR (10k-line diff, 15k-line file, 140k-line generated file): range limits, bounded event log, SQLite WAL under SSE + runner posts | developer-bilby, sonnet | ~300 |
| WP4-DOCS | README, runbook (GitHub App, Codex device login into the runner volume, `SR_SECRET_KEY`, backups, upgrade), CLAUDE.md commands | technical-writer-trillian, sonnet | docs |

Exit: owner acceptance session on the deployed stack against §13.

## 11. Traceability (brief v1 → work package → test)

| Brief item | WP | Test (name or kind) |
|---|---|---|
| §2 Four screens; loading/empty/error states | SCREENS, TOUR | Playwright per screen and state |
| §2 Input: PR URL, public repos only; errors | INGEST, SCREENS | `resolve_rejects_non_pr_url`, `resolve_rejects_private_repo`, Home error Playwright |
| §2 GitHub login, single tenant `default`, sign-in allowlist | AUTH (2+3) | login e2e; `tenant_default_seeded`; `allowlist_rejects_unlisted_login` |
| §2 Review opens only when finished | PIPE, SCREENS | Playwright: no Open review before finished |
| §2/§3 PR context (problem, related work, fit questions, CI, counters, profile status) | PIPE, PROMPTS, PREPASS, BLOCKS | pipeline fixture test; PR-context component test |
| §2 GitHub threads at steps with LLM status (from `analyse_flow`; others none); CI status | REVIEW, PIPE, INSPECTOR | thread placement and status tests; top-bar CI test |
| §2/§5 Ask (fresh on-demand job with context pack, pinned LLM answer, priority at claim) | ASK, INSPECTOR, E2E | `ask_context_pack_contents`, `ask_claimed_before_batch`, `ask_answer_stored_and_streamed`; Playwright Ask |
| §2 Ticks, flow reviewed, "flows reviewed k / n", last viewed block persisted | STATE, TOUR | review-state API tests; Playwright reload keeps position |
| §2/§5 Comments into GitHub pending review (create/reuse/edit/delete, count, list); outside the diff as file-level comment with line link | REVIEW, INSPECTOR, E2E | WP3-REVIEW tests incl. `outside_diff_comment_is_file_level`; Playwright comments |
| §2/§3 Feedback values + note; `missed by the LLM`; MemCan; calibration | MCP, REVIEW, INSPECTOR, E2E | feedback API tests; hard cases 1–2 |
| §2/§4.1 Resume; head moved banner; Re-analyze; open old analysis | PIPE, SCREENS, E2E | `head_moved_new_analysis_without_ticks`; Playwright banner |
| §2/§5 Project profile union, file wins; "no project profile" | PROMPTS, PIPE | `profile_union_file_wins`; `noprofile` state |
| §2/§5 Related repos from Cargo files, same org | PREPASS, MCP | `related_repos_same_org_only`; MCP scope test |
| §2 UI English; LLM texts in configured language | PROMPTS, STATE | prompt snapshot per language; settings test |
| §2/§5 `generic` plugin; path globs, line regexes; call hierarchy n/a | PREPASS, INSPECTOR | `generic_call_hierarchy_none`; "n/a" component test |
| §2 Huge PRs split into many runs | PIPE, PERF | `huge_pr_plan_splits_by_directory`; WP4-PERF |
| §2/§5 Codex CLI, provider trait, usage-limit pause/resume; model selection (Sol, high) | RUN, PIPE, SCREENS, INGEST | usage-limit JSONL test; paused state Playwright; `select_model_returns_sol_high` |
| §2/§5 Agent access: read-only checkout, MemCan (read and write), claudius, lookup tool, related repos | RUN, MCP, OPS | T1–T6 tests |
| §2/§5 Compose on LAN, Portainer-compatible | OPS | compose e2e; owner Portainer import (manual) |
| §1 Coverage invariant; Unexplained listed first among quiet rows | VERIFY | proptest; ordering test |
| §3 Classification list; Not analysed sections with counters | PREPASS, BLOCKS | classification tests; quiet-row component test |
| §3 LLM-classified refactor = normal flow with label, never collapsed | VERIFY, BLOCKS | ordering test; component test |
| §3 Test code inside production files: own flow at the end, "LLM-classified tests", never hidden | PROMPTS, VERIFY, BLOCKS | ordering test; component test |
| §3 FlowGraph; node ↔ step | VERIFY, DIAGRAM | graph consistency test; diagram click test |
| §3 PatternRepetition "same pattern × N" | BLOCKS | component test |
| §3 Fragment kinds incl. deleted site shown by default, moved badges | PREPASS, CODEVIEW | moves tests; code-view tests |
| §3 Neutral gutter on every changed line; LLM colour only on annotated ranges | CODEVIEW | code-view tests |
| §3 Assessment fields, collapsed by default | INSPECTOR, BLOCKS | component test |
| §3 Annotation types, evidence, fact vs inference | WP1-A, VERIFY | schema test; verify test |
| §3 Provenance badges (deterministic only) | WP1-A, BLOCKS | enum test; badge test |
| §4 Full width; PR number link; tooltip on every label; legend in `?`; top bar | TOUR, BLOCKS | Playwright tooltip sweep; top-bar test |
| §4.2 Progress: planning, running, paused, failed (incl. before any flow), finished; cancel discards partial results | SCREENS, PIPE, JOBS | Playwright per state; `cancel_discards_partial_results` |
| §4.3 Sequence and flow order; Previous/Next; keyboard map | TOUR, VERIFY | Playwright keyboard; ordering test |
| §4.3 Columns resizable, hide, full screen, remembered | TOUR | Playwright layout persistence |
| §4.3 Diagram hidden by default; one block at a time | TOUR, DIAGRAM | Playwright |
| §4.3 Inspector never empty; sections order; collapse remembered | INSPECTOR | Playwright per block type |
| §4.3 Show old code; classic diff | STATE (`diff`), CODEVIEW | diff endpoint test; `d` key Playwright |
| §4.3 Three hard cases; no profile; interrupted review | E2E, TOUR | §12.4 |
| §4.4 Settings: credential check on demand, language, App status, per-repo read-only; first Codex login via runbook | STATE, RUN, AUTH, SCREENS, DOCS | `llm_check_requests_runner_check`; Playwright |
| §5 `JobSpec`/`JobExecutor`/`execute`; job kinds; caches optional | WP1-A, JOBS, RUN | A3/A4 tests |
| §5 Pipeline steps 1–6, up to 2 repairs | INGEST, PIPE, VERIFY | WP3-PIPE tests |
| §5 Lookup MCP tool read-only, user token, scoped | MCP | WP3-MCP tests |
| §5 Multi-tenancy columns, seeded tenant | WP1-A | migration test |
| §5 Secrets encrypted; cookie sessions; CSRF; trusted proxy | AUTH (2+3) | WP2/3-AUTH tests |
| §5 Frontend: self-contained components | all UI WPs | ESLint A7 |
| §7 Large files: range API, displayed-lines Shiki, generated never given, ranges in prompts | INGEST, CODEVIEW, PREPASS, PROMPTS, PERF | cap test; WP4-PERF |

## 12. Verification strategy

### 12.1 Per stage
| Stage | Unit | Contract | End-to-end / real |
|---|---|---|---|
| 0 | — | — | real Codex (S1a, S7), real GitHub (S3, S4) |
| 1 | contract serde, schema rules | drift test, fixture round-trip | schema smoke per job kind (real Codex) |
| 2 | per WP | server golden tour; fake-github contract | full-stack slice Playwright; manual real-Codex slice smoke; owner checkpoint 1 |
| 3 | per WP | golden Codex JSONL; route ↔ msw handler test | Playwright six states (msw) + full-stack; manual real-GitHub smoke; owner checkpoint 2 |
| 4 | — | real outputs validate | WP4-REAL, WP4-PERF, acceptance session |

### 12.2 Independent review
Every WP with ≥50 code lines: `claudius:review-changes` basic mode (one strong reviewer, opus) before merge;
findings fixed by the same agent; merge classes per `claudius:severity`. Stage branches: review of merge
resolutions only.

### 12.3 Security
Design before the freeze (WP1-T, §5.4); full-mode reviews with security pass for WP2-AUTH, WP3-AUTH,
WP3-REVIEW, WP3-MCP, WP3-RUN, WP3-OPS; audit at the end (WP4-SEC).

### 12.4 Trust, wording and the hard cases as tests
- **Rendered text** (Playwright, all six states and all screens): page text never contains "no risk",
  "nothing to review", "no review needed", "noise", "verified", "Was wrong", "confidence", a percentage, or
  "%" in the top bar. `scripts/check-wording.sh` greps `web/src` string literals only (prompts must name the
  banned words, so they are not grepped).
- **Agent output**: prompts forbid the banned phrases and numeric likelihoods; WP3-VERIFY logs a warning when
  LLM text contains them (no hard rejection); schema forbids confidence fields; facts need evidence.
- **Components**: "LLM analysis unless marked otherwise" once per LLM panel; Assessment collapsed by default;
  highlight tooltip = was → is only; no questions → "The LLM raised no questions. That is not evidence of no
  defects."; LLM-classified refactors as normal flows with the label; green tick only on deterministic badges;
  every badge, status, button and column header has a tooltip (Playwright sweep); inspector never empty.
- **Ordering**: behavioural flows, then LLM-classified refactors, then LLM-classified tests, then quiet rows;
  quiet rows never above a rated flow; Unexplained first among quiet rows.
- **Hard case 1** (confident wrong HIGH): mark `wrong` + `overrated` → stored; fake MemCan gets a calibration
  example. **Hard case 2** (defect on a plain changed line): comment on an unannotated changed line → fake
  GitHub gets the thread; `missed_by_llm` stored; comment marker shown. **Hard case 3** (`noq`): required
  sentence, no invented risk. Plus "no project profile" and "interrupted review".

## 13. Definition of done for v1

- All rows of §11 have a passing test; the six mockup states pass in Playwright; owner checkpoints 1 and 2
  approved.
- A real public PR analysed end to end on the compose stack with Codex Pro; pause/resume, cancel, retry,
  resume, re-analyze work.
- Comments land in the user's pending review (create, reuse, edit, delete); feedback stored locally and in
  MemCan; Ask answers pinned and labelled.
- A1–A12 checks green in CI; WP4-SEC has no open blocking finding; drift test green.
- The runbook deploys the stack on Portainer from scratch; secrets never reach the browser or logs.

**Not in v1** (brief §2 Later, Non-goals): call hierarchy content (trait method and API field exist,
`generic` returns `None`; tree-sitter-tags language plugins later, no rust-analyzer); Rust and Go tech plugins
(until then test code inside production files is LLM-classified and shown as its own flow); Claude provider;
private repositories; per-job containers
(`ContainerExecutor`, one-shot runner entry); Groups workspace, overview, files tree; partial review while
running; re-review with carried ticks; recent-reviews list; dashboard; LLM auto-comment; PR-description-vs-code
check; static-analysis actions; GitHub "Viewed" sync; custom "not analysed" rules; tenant UI, real tenants,
billing; automatic approve/reject; local comment drafts; PR-level risk or size statistics; numeric confidence.

## 14. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Codex cannot produce the agent-output shape reliably | S7 before the freeze; schema smoke at M1; real slice smoke at M2; WP4-REAL |
| Codex CLI format changes | Pinned CLI version; golden JSONL tests; unknown events tolerated |
| Token rotation with concurrency | One job per credential; `runner` uid is the single `auth.json` writer (T1) |
| Prompt injection from PR content | T1–T6; read-only checkout; egress allowlist; WP4-SEC |
| Agents drift from trust wording | Rendered-text and agent-output checks; closed enums; A9 |
| Contract churn during parallel work | Freeze after owner merge; CCR; drift test; pre-declared state/config/deps |
| Fake GitHub diverges from github.com | Fixtures recorded in S3/S4; manual real-GitHub smoke at M3b |
| UI looks wrong to the owner | Ported mockup tokens; Diziet review; owner checkpoints at M2 and M3a |
| Host OOM | One heavy job incl. Docker builds; web worker caps; ≤3 Rust agents at once |
| Huge PRs / files; DB growth | Range-capped API; generated files excluded; bounded event log; WP4-PERF |
| SQLite contention | WAL, one write pool in `db`, short transactions; WP4-PERF |
| ToS of subscription use | Single tenant, sign-in allowlist (owner only by default); `api_key` credential type kept |
| Memory poisoning via agent MemCan writes | Accepted by the owner (T6); server-side feedback writes unaffected; future MemCan access control |
| TS 7 / Vite 8 / msw 3 churn | Verified in WP2-WEB; fallback to penny's versions (ADR) |

**Brief §8 open points**: pending-review API → S3 → WP3-REVIEW; Codex sandbox in container → S1b → WP3-RUN,
WP3-OPS; neutralising repo config → Codex: S1b → WP3-RUN, Claude: with the Claude provider (not v1);
Portainer and containers created outside the stack → not v1 (WP3-OPS checks only the v1 stack import); Rust/Go
plugin timing → decided: v2; file-level pending comments → S3 → WP3-REVIEW; MemCan token and scope → O5, S5
(§16 Open).

## 15. Agents and models

| Work | Agent type | Model |
|---|---|---|
| Spikes; contracts; jobs, auth, ingest, runner, review, pre-pass, verify, pipeline, prompts, Ask, MCP; code view; Tour | `claudius:developer-bilby` | opus |
| Skeleton, fixtures, CI, fake GitHub/MemCan, web scaffold, diagram, blocks, inspector, screens, state API, ops, e2e, perf | `claudius:developer-bilby` | sonnet |
| Contract and CCR reviews | coordinator with `claudius:architecture`, or `claudius:architect-nagatha` | opus |
| Threat model, security audit | `claudius:security-engineer-smythe` | opus |
| Independent reviews | `claudius:review-changes` (correctness pass: `claudius:qa-engineer-marvin` dispatched with opus) | opus |
| UX review at M3a | `claudius:ux-designer-diziet` | opus |
| Docs | `claudius:technical-writer-trillian` | sonnet |

Every brief: §2 block, goal, owned paths, inputs (paths), acceptance tests, worktree path + SHA, MemCan prior
knowledge, skills (`claudius:coding-best-practices` always; `claudius:security-best-practices` for security
WPs), reporting channel. Brief WHAT, not HOW. WPs over ~1,000 lines are split before dispatch.

## 16. Owner decisions (2026-11-16)

| Topic | Decision | Shaped |
|---|---|---|
| GitHub auth | GitHub App user tokens via the owner's existing App; installing it on target repos (dashpay org) is fine; S4 verifies behaviour | §6.1 O2, S4, WP3-AUTH |
| Repository | `lklimek/smart-review` public; GitHub Actions enabled | §1, WP1-C |
| Codex model | Sol at reasoning effort high for every job kind; `select_model(kind, context)` placeholder, later a real decision function; exact id verified in S1a | §5.2, S1a, WP2-INGEST |
| MemCan from the agent | Full MemCan MCP (read and write); memory poisoning accepted for now; server writes feedback and calibration | §5.4 T6, WP3-RUN, §14 |
| Quota | No cap on the Pro plan; real runs purposeful | §6.1 |
| Comment outside the diff | File-level comment in the pending review, body links to the line (S3 confirms) | S3, WP3-REVIEW, WP3-INSPECTOR |
| Sign-in | Allowlist of GitHub logins in config; default the owner only | WP3-AUTH |
| Ask | Fresh agent run per question with a context pack (step data, flow analysis, earlier Q&A of the step); no session resume; `LlmProvider` without `resume` | §5.3, S1b, WP3-ASK, WP3-RUN |
| Progress before n is known | Indeterminate bar, "Planning flows…" | WP3-SCREENS |
| Failure before any flow | Failed state with a plain description and Retry | WP3-PIPE, WP3-SCREENS |
| Cancel | Partial results discarded; Home shows "Analysis cancelled — analyze again" | WP3-PIPE, WP3-SCREENS |
| First Codex login | Manual one-time command in the runner container (runbook); no UI | WP4-DOCS |
| "LLM working" in Settings | Cheap check by the runner on demand ("Check again") | §5.3, WP3-STATE, WP3-RUN |
| LLM status of threads | From `analyse_flow` for threads located in its steps; other threads have no status | WP3-PIPE, WP3-PROMPTS |
| Ask during re-analysis | Claimed before the next batch job; a running job is not pre-empted | WP3-ASK |
| Numbers like "80% likely" | Forbidden in the prompt; no hard rejection | WP3-PROMPTS, WP3-VERIFY |
| Test code inside production files | LLM-classified; own flow at the end labelled "LLM-classified tests"; never hidden. Rust/Go plugins in v2 | WP3-PROMPTS, WP3-VERIFY, WP3-BLOCKS, §13 |
| Stage PRs | Each stage a PR merged by the owner; the next stage may start stacked on it | §1 Git flow |
| Visual checkpoints | Screenshots after the slice (M2) and after the UI (M3a) | §8, §9 |

**Open**: whether the runner needs a MemCan token, and the MemCan `project` scope for calibration examples
(working assumption: no token on the LAN, scope = repository short name). Ask with O5; S5 verifies reachability.

## Appendix A. Review dispositions (`/data/artifacts/smart-review/2026-11-16/reviews/plan-review-fable.md`)

| Finding | Disposition |
|---|---|
| B1 | Added S7 (real PR, draft schemas) in the stage-0 gate; schema smoke per job kind in WP1-A acceptance; minimal Codex `run` in WP2-RUN and a manual real-Codex slice smoke at M2. |
| B2 | Added §11 traceability; new WP3-STATE (review state, settings, diff, heartbeat, runner status) and WP3-E2E (full-stack extension, fake MemCan); threads/checks repos to WP3-REVIEW; `analysis` public signatures moved to WP1-A. |
| M1 | Split WP2-CORE (JOBS, AUTH, INGEST), WP3-GH (AUTH, REVIEW), WP3-PRE (PREPASS, VERIFY), WP3-PIPE (PIPE, PROMPTS, ASK), WP3-UI-CODE (CODEVIEW, DIAGRAM), WP3-UI-COMP (BLOCKS, INSPECTOR), WP3-UI-SCREENS (SCREENS, TOUR); WP1-S split from WP1-A; slice scope stated as thin. |
| M2 | S5 in the gate; S1 split into S1a/S1b; pause for the owner's device login; host `~/.codex` forbidden. |
| M3 | Hand-written: repo trees, GitHub responses, agent goldens, slice-only API seed; API default generated by the server at M2; other states as merge-patch overlays; endpoint × state inventory; route ↔ msw handler test; round-trip comparison instead of `deny_unknown_fields` (simpler, no attribute on every type); 14 steps decided; fixture ownership in §4.3. |
| M4 | §4.3 collision table incl. lockfiles, manifests, `lib.rs`, `AppState`/`Config`, mocks; skeleton pre-declares deps/state/modules; fake-github contract test moved to server tests; `embed-ui` feature chosen over a committed empty `web/dist` (no build-output churn); configurable clone base + `file://`/`git daemon` remote; `e2e/**` owned sequentially by WP2-E2E then WP3-E2E. Hidden dependencies in the Depends column. |
| M5 | Standing block covers Docker builds, `CARGO_BUILD_JOBS=2`, web worker caps, slot-wait re-run. Simpler than "only the coordinator builds images": the ops agent builds when the coordinator grants the slot. |
| M6 | §5.4 security design and WP1-T threat model before the freeze; tests in WP3-RUN/WP3-OPS; compensating controls for sandbox-off; A10 corrected (runner secret). |
| M7 | S3/S4 record sanitised responses for the fake; manual real-GitHub smoke at M3b. |
| M8 | First session commits plan + CLAUDE.md; owner creates remote/base; git flow with gates, one-deep stacking, PR size target, stage-3 PR split at M3a/M3b/M3c. |
| M9 | Mockup tokens ported in WP2-WEB; Diziet UX review and owner checkpoints at M2 and M3a. |
| M10 | `basis` (fact or inference) + evidence rule in contracts and verify; rendered-text checks in Playwright; grep limited to `web/src`; tooltip sweep, Unexplained-first and inspector-never-empty tests. |
| m1 | Agent type names corrected. |
| m2 | Tracking file is the single record; MemCan for decisions and owner questions only; stage review covers merge resolutions; job schemas dropped. |
| m3 | Decided in §5.2 (ADR-001): runtime sqlx queries, npm, `SR_SECRET_KEY`, port/base URLs, `select_model` placeholder (Sol, high). |
| m4 | Drop list next to the `prior-art.md` reference (§7). |
| m5 | "Changed line without annotation" in WP3-REVIEW goal and test. |
| m6 | `lib.rs`/`main.rs` stubs owned by WP1-S. |
| m7 | Server-generated golden replaces "fields that exist so far". |
| m8 | Spike Dockerfile, compose, JSONL and scripts archived with the evidence. |
| Missing questions 1–8 | Asked; answered by the owner (§16). |
