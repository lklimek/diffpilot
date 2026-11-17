# smart-review — implementation plan (v1)

Status: implementation plan, 2026-11-16. Authoritative design: `docs/design-brief.md` (cited as "brief §N").
This plan says HOW v1 is built and by whom; it does not change the design. The owner's decisions are in §16
and are reflected in the brief. The owner is away during implementation (§1 Owner session, Authority).
One-page diagram: `docs/architecture.html`.

## 1. Start here (coordinator, fresh session)

You coordinate and guard the architecture. You do not write application code; every code change is made by a
delegated agent in its own worktree. You talk to the owner in Polish; everything in the repo or on GitHub is
English.

**Read, in this order**
1. `CLAUDE.md`, then this file §1–§5 completely, then §11 (traceability) and §16 (owner decisions).
2. `docs/design-brief.md` completely. Mockups: `docs/mockups/index.html`; `05-tour.html` + `mockup.js` hold the
   example PR `acme/node-client#482` used by every fixture.
3. `docs/decisions/` (ADRs), `docs/spikes/` (spike findings), `docs/contracts/` — once they exist.
4. Background outside the repo, only when a work package needs it (`/data/artifacts/smart-review/2026-11-16/`):
   `research/llm-runtime.md`; `research/prior-art.md` (§5 candidate JSON — see the drop list in §7);
   `research/egress-proxy.md` with `research/egress-proxy-squid-tested.conf` and
   `research/egress-proxy-squid-tested.Dockerfile` (Squid setup for S6 and WP3-OPS; its SSL-Bump, CA and cache
  parts are the later interception step, not v1); `reviews/psych-fable.md`,
   `reviews/psych-astra.md`.

**Load skills** before the first dispatch: `claudius:grand-admiral`, `claudius:delegate` (incl. its external
Codex launcher reference for reviews), `claudius:git-and-github`, `claudius:coding-best-practices`;
`claudius:architecture` for contract reviews and change requests; `claudius:review-changes` at every gate.

**Tracking** — one record: `/data/artifacts/smart-review/implementation-tracking.md` (outside git, survives
reboot). One row per work package (WP): `WP | status (todo/dispatched/review/merged/blocked) | branch |
worktree | agent id | base SHA | review report | notes`, plus a section **For the owner** listing every ADR,
deviation from brief or plan, accepted fallback and owner-only follow-up as it happens; at the end it becomes
`docs/v1-handover.md`. Update before and after every dispatch, review and merge. MemCan
(`project="smart-review"`) holds durable decisions only. Review reports and spike evidence: copy to
`/data/artifacts/smart-review/YYYY-MM-DD/` right after each round.

**Resume after an interruption**
1. Re-read the tracking file; `search(query="smart-review implementation", project="smart-review")`.
2. Reconcile with reality: `git branch -a`, `git worktree list`, `git status` in every worktree under
   `/data/git-worktrees/home-ubuntu-git-smart-review-*`. Git state wins over the file.
3. Worktree with uncommitted work and no live agent: dispatch a fresh agent into the same worktree with the
   original brief plus "first review the existing uncommitted diff, then continue". Never delete it.
4. Continue with the earliest stage whose exit gate is not met.

**Owner session (once, before any agent is dispatched)** — the only step that needs the owner; nothing after
it waits for the owner.
1. Prepare alone first: build the spike Codex container (pinned Codex CLI, non-root) and create its
   `CODEX_HOME` volume (§6.1); have the items of §6.1 ready in one Polish message.
2. With the owner, collect O1–O6 (§6.1), including the Codex device login and one real GitHub App login.
   Confirm each item works before the owner leaves.
3. Commit `CLAUDE.md`, this plan and the diagram on `docs/initial-design` if not yet committed.

**Authority while the owner is away**
- *May decide alone* (record an ADR and a "For the owner" entry): spike fallbacks, contract approval, merges,
  visual-conformance and GUI-test decisions, any choice inside v1 scope that the brief leaves open, a
  security fallback that keeps or strengthens T1–T9, deployment of the stack per §10.
- *Must not*: push to a base branch (`main`); open pull requests or publish anything other than `feat/v1`;
  change the v1 scope (brief §2); weaken a security control T1–T9 without recording it as a deviation; spend
  on paid APIs; change the MemCan deployment (its IP allowlist or token) or the owner's reverse proxy.
- When blocked on something only the owner can do: record it, notify with the `summon` skill (non-blocking),
  and continue with all work that does not depend on it (for example on the fake provider).

**Git flow** — one branch, no pull requests until v1 is deployed.
- `lklimek/smart-review` exists and is public; the owner pushes `main` himself. Feature branch `feat/v1` is cut
  from the tip of `docs/initial-design`. WP branches are cut from `feat/v1` at a recorded SHA, each in a
  pre-created worktree `/data/git-worktrees/home-ubuntu-git-smart-review-<wp>`.
- The coordinator merges WP branches into `feat/v1` (merge, no rebase, no amend, no force-push), verifies
  (§4.2) and pushes `feat/v1` itself (`ghsudo git push`) as it progresses. Never `main`.
- Until v1 is deployed there is no compatibility to keep: no versioning, no changelog, the schema is one
  `0001_init.sql` that is rewritten in place, and contracts may be reshaped by change request (§4.3).

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
  Touch only your "Owned paths" plus the shared files of plan §4.3 under its rules. Contract, schema,
  route, trait or fixture-shape changes: stop and report a change request.
- Real dashpay PRs (plan §6.1): only pending review comments, deleted afterwards; never submit a review.
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
crates/db/          THE database module: sqlx/SQLite, `0001_init.sql`, repository API (one file per aggregate).
crates/server/      axum app: auth, sessions, secrets, GitHub (REST/GraphQL, bare mirror, installation-token
                    minting), pipeline, prompts, `select_model`, Ask, JobExecutor + QueueExecutor, runner
                    protocol, SSE, MemCan client, SPA embedding (feature `embed-ui`, off in dev/test builds).
crates/runner/      runner: execute(JobSpec, EventSink) -> JobResult, polling loop, LlmProvider + Codex provider
                    (+ fake provider behind feature `fake-provider`), per-job Codex config (github-mcp-server
                    read-only, MemCan), agent-session store, claudius checkout refresh, mirror cache.
tools/fake-github/  dev/test only: fake GitHub REST/GraphQL/OAuth serving recorded fixtures. Never in images.
tools/fake-memcan/  dev/test only: fake MemCan MCP. Never in images.
web/                React SPA: src/api/generated/ (checked in), src/api/client.ts (only fetch layer),
                    src/components/<block>/, src/screens/<screen>/, src/mocks/ (msw), tests/ (Playwright vs msw).
e2e/                full-stack Playwright (server + runner + fake GitHub + fake MemCan).
prompts/            agent prompt templates per job kind.     schemas/agent/   generated agent-output schemas.
fixtures/pr-482/    example PR from the mockups (§7 WP1-B).  deploy/  Dockerfiles, compose, Squid egress proxy.
docs/               brief, plan, architecture.html, contracts/, decisions/, spikes/, runbook, v1-handover.
```

```
            contracts  <──────────────┬───────────────┬──────────────┐
                ^                     │               │              │
   analysis ────┘          db ────────┘     runner ───┘   tools/fake-* (dev only)
       ^                    ^
       └──── server ────────┘   (server -> contracts, analysis, db)
   web -> web/src/api/generated (produced from contracts); never imports server code
```

Nothing depends on `server` or `runner`. `runner` never depends on `db`, `analysis` or GitHub crates (it only
launches the external `github-mcp-server` binary for the agent). `sqlx` only in `crates/db/Cargo.toml`;
GitHub client code and the App private key only in `server::github`. Enforced by
`scripts/check-architecture.sh` (reads `cargo metadata`) in CI from stage 1.

## 4. Guarding the architecture

### 4.1 Rules
"Guarding" = the coordinator sends back any merge that breaks a rule, however good the code is otherwise.

| # | Rule (brief ref) | Check |
|---|---|---|
| A1 | Dependency direction of §3 | `check-architecture.sh` in CI |
| A2 | All DB code in `crates/db`; its public API exposes no sqlx types (brief §6) | A1 script + review |
| A3 | Pipeline and Ask talk only to `JobExecutor` (`submit`, `events`, `cancel`, `cancel_analysis`); `QueueExecutor` is the only v1 impl; no server code assumes a long-lived runner or a shared filesystem (brief §5 items 1–5) | review; grep server for runner paths/volumes |
| A4 | `JobSpec` self-contained; runner core is `execute(JobSpec, EventSink) -> JobResult`; loop is a thin wrapper; a job succeeds with an empty cache and an empty session store | runner tests with empty cache and store |
| A5 | Only `runner::provider` knows Codex flags, events, config, resume and compaction; the rest sees normalised events and typed errors | review |
| A6 | Only `analysis::tech` knows path globs / line regexes; `TechPlugin` = `detect`, `classify`, `call_hierarchy -> Option<..>`; `generic` returns `None` | unit test |
| A7 | Every UI block is a self-contained component: props in, callbacks out; no fetching, router hooks or screen imports in `web/src/components/**`; screens only arrange (brief §5 Frontend structure) | ESLint `no-restricted-imports` |
| A8 | Contract types only from `crates/contracts`; no hand-written TS duplicates; generated files current | drift test (§5) |
| A9 | Trust and wording rules (brief §1, §4.3) | tests of §12.4 |
| A10 | Security design of §5.4 (T1–T9) | WP3-RUN/WP3-OPS/WP3-APPTOKEN tests, security reviews |
| A11 | Rust conventions of §2 | `[workspace.lints]` (`unwrap_used`, `expect_used`, `allow_attributes` deny), `clippy.toml` `allow-unwrap-in-tests`, CI grep for anyhow/eyre |
| A12 | Simplicity: no trait, crate, service or config option the brief does not name, unless an ADR says why; `select_model` stays a plain function | review |

### 4.2 Per-merge checklist (WP branch → `feat/v1`)
1. Diff touches only owned paths and §4.3 shared files under their rule (`git diff --stat`).
2. Report has the ledger line for the current tree and names the new tests; each acceptance item maps to a test.
3. A1–A12 hold — read the diff, not the summary.
4. Independent review per `claudius:review-changes` for ≥50 code lines, reviewer = Codex Astra (§15; effort
   xhigh for security-sensitive WPs, high otherwise). Blocking findings go back to the same agent; the
   reviewer re-checks. Report archived.
5. Generated artefacts current (drift test in the ledger).
6. After the merge: one targeted test run of the touched crates/packages on the merged tree. At each merge
   point (M1, M2, M3a, M3b, M3c) a review of the merge resolutions (Astra, xhigh). Update tracking; push.

### 4.3 Shared files (collision points)
| File | Rule |
|---|---|
| `Cargo.toml` (workspace deps) | All §5 crates pre-declared in WP1-S; a WP adding a crate adds one line. |
| `Cargo.lock`, `web/package-lock.json` | Agents commit their lock changes; on conflict the coordinator takes `feat/v1`'s version and lets `cargo metadata` / `npm install` re-resolve minimally (no `cargo update`). |
| `crates/*/Cargo.toml` | Add-only lines for the WP's own deps. |
| `crates/server/src/lib.rs`, `crates/db/src/lib.rs`, `crates/analysis/src/lib.rs` | Module lines pre-created in WP1-S; agents do not edit them. |
| `crates/server/src/state.rs` (`AppState`), `crates/server/src/config.rs` | All fields pre-declared in WP1-S from §5; a WP fills only its own constructor/validation; new fields = change request. |
| `crates/server/src/api/mod.rs` | One `.merge(<module>::routes())` per module, pre-created in WP1-S. |
| `crates/db/migrations/0001_init.sql` | Single schema file; edits only through a change request. |
| `web/package.json`, `web/src/app/routes.tsx` | Add-only lines. |
| `web/src/mocks/handlers/<endpoint>.ts` | One file per endpoint, owned by the WP that owns the endpoint's UI consumer. |
| `fixtures/pr-482/**` | Shape changes are change requests; content changes belong to the WP whose producer changes (§7). |

**Change request (CCR).** Contracts, the schema, routes, trait signatures, `AppState`/`Config` fields and
fixture shapes are frozen after stage 1 so parallel agents do not collide. An agent needing a change stops and
reports. The coordinator decides (architecture skill), applies it in one small commit on `feat/v1` (or a short
WP), regenerates artefacts, writes an ADR if not trivial, and tells running agents to merge the new SHA. No
backward compatibility is kept before deployment.

## 5. Contracts

### 5.1 Source of truth and drift prevention
| Contract | Source of truth | Generated (checked in) | Drift guard |
|---|---|---|---|
| Agent output per job kind (`plan`, `pr_context`, `analyse_flow`, `ask`) | `crates/contracts/src/agent/` (serde + `schemars`) | `schemas/agent/<kind>.schema.json`, Codex-strict subset (S1a, S7) | test `generated_artifacts_are_current` regenerates and compares; `UPDATE_GENERATED=1` rewrites; CI also `git diff --exit-code` |
| HTTP API DTOs, error model, SSE events | `crates/contracts/src/api/` (serde + `ts-rs`, gamiscreen's `ts` feature pattern) | `web/src/api/generated/*.ts` | same test |
| Routes | `crates/contracts/src/api/routes.rs` (path constants + method) | `web/src/api/generated/routes.ts` | same test; server router built from the constants; vitest: every route has an msw handler |
| Runner protocol, `JobSpec`, `JobEvent`, `JobResult`, state machines | `crates/contracts/src/job/` | — | Rust tests |
| Database | `crates/db/migrations/0001_init.sql` (rewritten in place until deployment), repository API `crates/db/src/repo/*.rs` | `docs/contracts/db-schema.md` (summary) | migrate-empty-DB test; CCR |
| Seams | `JobExecutor` (`server::jobs`), `LlmProvider` (`runner::provider`), `TechPlugin` + verify/coverage/ordering/moves/related-repo signatures (`analysis`) | — | review (A3–A6) |
| Fixtures | `fixtures/pr-482/` | API golden generated by the server from M2 | §7 WP1-B tests |

Versions checked on the registries 2026-11-16 (re-verify when adding): `schemars` 1.2.3, `ts-rs` 12.0.1,
`jsonschema` 0.61.2, `axum` 0.8.9, `sqlx` 0.9.0 (penny: 0.8), `rmcp` 3.10.0 (client only, MemCan),
`octocrab` 0.54.9, `aes-gcm` 0.11.1, `rust-embed` 8.14.0, `similar` 3.2.1, `wiremock` 0.6.5, `proptest` 1.12.0,
`insta` 1.49.0; npm `react` 19.3.1, `vite` 8.3.8, `@tanstack/react-query` 5.107.1, `react-router` 8.5.1,
`tailwindcss` 4.3.5, `shiki` 4.6.0, `msw` 3.0.6, `vitest` 5.1.0, `@playwright/test` 1.65.1, `typescript` 7.0.2 (if
tooling breaks: penny's 5.9 line, recorded in an ADR). `github-mcp-server`: version pinned in S5.

### 5.2 Decisions fixed in stage 1 (ADR-001, WP1-S/WP1-A)
- sqlx runtime queries (`query`/`query_as`, as penny; no macros, no offline data); npm with
  `package-lock.json` (as penny and gamiscreen); AES-GCM key from env `SR_SECRET_KEY` (32 bytes, base64,
  created by the runbook; no rotation in v1).
- Configuration (env): `SR_PUBLIC_URL` (public base URL; the owner's instance `https://review.klimek.ws`,
  callback `…/auth/github/callback`); server listens on HTTP port 3000 behind the owner's TLS reverse proxy,
  with a trusted-proxy setting and Secure cookies; no built-in TLS or ACME. GitHub App: `SR_GITHUB_APP_ID`,
  client id and secret, `SR_GITHUB_APP_PRIVATE_KEY_FILE` (.pem, read-only mount). MemCan: `SR_MEMCAN_URL`,
  `SR_MEMCAN_TOKEN` — pure configuration, nothing assumes the current MemCan host (it moves before
  deployment). Configurable GitHub API, OAuth and clone base URLs (dev: fake GitHub and a `file://` or
  `git daemon` remote).
- Model selection is a plain Rust function in the server, `select_model(job kind, context) -> ModelChoice {
  model, reasoning_effort }` in `server/src/model.rs`, returning Sol at high effort for every kind in v1
  (exact model id from S1a). No table, no setting.

### 5.3 Design rules for the contracts (binding for WP1-A)
- Closed enums exactly as brief §3 (classification, fragment kind, annotation type, feedback values, node kind,
  provenance badges `text_identical` / `classified_by_path` / `classified_by_line_rule`).
- No `confidence`, score, percentage or float anywhere; no PR-level risk. A schema test asserts that the only
  numbers are integer line numbers and counters.
- Every annotation and observation carries `basis: fact | inference`; `fact` requires ≥1 evidence reference
  (`file`, `line`); verify rejects a fact without evidence (brief §1 "observed facts carry a line reference").
- Lines are 1-based inclusive at the head SHA; ids are strings stable within one analysis.
- **Agent sessions** (brief §5): `JobSpec` carries `ModelChoice` and `session: Option<SessionRef>` (analysis id
  + provider session id) plus a `context_pack` used only when a new session must be started; `JobResult`
  returns the session id and `session_outcome: resumed | started | started_after_loss`. `LlmProvider` =
  `run`, `resume`, `cancel`; `run` and `resume` end by compacting the session (provider-specific, S1b). The
  server allows one running job per session; a re-analysis starts a new session.
- **Agent credentials in `JobSpec`**: `github: InstallationToken { token, expires_at }` (read-only, PR
  repository only per ADR-005, 1 h) and `memcan: { url, token }`; the runner turns them into the per-job Codex config
  (github-mcp-server read-only; MemCan MCP). No other credential reaches the agent.
- Agent schemas use the Codex structured-output subset found in S1a/S7; the server re-validates with
  `jsonschema`.
- Call hierarchy is in no agent schema. API `Step.call_hierarchy: Option<CallHierarchy>` is filled by the
  server from `TechPlugin::call_hierarchy`; `None` in v1 (UI: "n/a — needs a language plugin").
- Deterministic facts (gutter marks, badges, counters, "N lines differ", changed-line sets) are server-set API
  fields in types separate from LLM content.
- Storage: validated agent output as JSON documents (PR context; one per flow); relational tables only for
  queue and mutable state: tenants, tenant settings (allowed sign-in logins; seeded `default` → `lklimek`),
  users, memberships, sessions, encrypted tokens, repos, pull requests, analyses (incl. agent-session id),
  jobs (+ bounded event log), threads/checks snapshot, review state, questions, feedback, comment idempotency
  keys, settings, last LLM call outcome. Every table has `tenant_id`; tenant `default` seeded (brief §5).
- Error model: `ApiError { code: ApiErrorCode, message }`, `message` = plain user-facing text, never internal
  error text; status mapping in `docs/contracts/http-api.md`.

**HTTP API sketch** (finalised in `docs/contracts/http-api.md` with an endpoint × fixture-state inventory):
`/auth/github/login|callback|logout`, `GET /api/v1/me`; `POST /api/v1/pr/resolve` (URL → PR, existing
analysis, head moved); `POST /api/v1/analyses`; `GET …/analyses/{id}`; `GET …/{id}/events` (SSE);
`POST …/{id}/cancel|retry`; `GET …/{id}/tour`; `GET …/{id}/lines?path&from&to` (range-capped, brief §7);
`GET …/{id}/diff?path&from&to`; `GET|PUT …/{id}/review-state`; `POST …/{id}/steps/{step}/questions`;
`POST …/{id}/feedback`; `GET …/{id}/pending-review`, `POST|PATCH|DELETE …/pending-review/comments` (idempotency
key); `GET /api/v1/settings` (incl. LLM status = outcome, time and plain message of the most recent LLM call),
`PUT …/settings/language`; `GET /api/v1/repos/{owner}/{name}/settings`. Mutating routes require the CSRF header.

**Runner protocol sketch**: `POST /runner/v1/claim` (runner secret) → `JobSpec` (with installation token and
MemCan credentials) + job token; `POST /runner/v1/jobs/{id}/events` (job token; response carries
`cancel_requested`); `POST …/{id}/result` (job token, idempotent per attempt; carries the provider call outcome
for the LLM status). An expired lease is the liveness signal. Job states `queued → claimed → running →
succeeded | failed | cancelled | paused{resumes_at}`; expired lease → `queued`, attempt + 1, bounded.

### 5.4 Security design (WP1-T, reviewed with the contracts; audited again in stage 4)
The process model behind T1 (supervisor, uids, private socket) is decided in ADR-013.

| # | Requirement | Tested in |
|---|---|---|
| T1 | Two uids in the runner image: `runner` owns runner secret, job token, mirror cache, claudius checkout, master `CODEX_HOME`, session store; Codex, `github-mcp-server` and every agent command run as `agent` with a fresh work dir, a per-job `CODEX_HOME` (generated config + copy of `auth.json` + this analysis's session files), read-only skills. `runner` copies `auth.json` and the session files back after the job (one job per credential and per session, so no race). | WP3-RUN, WP3-OPS |
| T2 | Agent environment scrubbed: only home, PATH and proxy variables (research `egress-proxy.md` §3; no CA variables in v1) and the variables the per-job config needs (installation token for `github-mcp-server`, MemCan token). | WP3-RUN |
| T3 | GitHub credentials: user tokens and the App private key never leave the server (key file mounted read-only, 0400). Per job the server mints an installation token with read-only permissions (pull requests, issues, contents, metadata), restricted to the PR repository only (ADR-005; related repositories are read as public data), lifetime 1 h — the only GitHub credential the agent holds; `github-mcp-server` runs in read-only mode. The job token never reaches the agent. | WP3-APPTOKEN, WP3-RUN |
| T4 | Container: non-root, `cap_drop: ALL`, `no-new-privileges`, read-only root fs except work dir and per-job `CODEX_HOME`, no Docker socket; runner on an internal network whose only egress is the Squid proxy (explicit proxy, no transparent mode); private LAN ranges denied except MemCan (SSRF guard, research §3); no TLS interception in v1 (no own CA anywhere); proxy log (host names) with restricted access and retention. If S1b disables Codex's own sandbox, T1–T4 are the compensating controls (ADR, "For the owner"). | WP3-OPS |
| T5 | Repository-provided agent config (AGENTS.md, `.codex/`, hooks, MCP config) never loaded (S1b mechanism). | WP3-RUN |
| T6 | MemCan: reached at its Traefik entrypoint (IP allowlist + bearer token). **Accepted risk** (owner): the agent has full MemCan MCP (read and write) with the single shared `MEMCAN_API_KEY`; an injected PR may write misleading memories. Per-client tokens and agent-written-memory separation are future MemCan work. Feedback and calibration examples are written by the server. | WP3-RUN, WP3-MEMCAN |
| T7 | Server: cookie session (Secure, HttpOnly, SameSite) behind the trusted reverse proxy, CSRF, tokens encrypted with AES-GCM, sign-in only for logins the tenant allows, nothing secret in logs, errors or the browser. | WP2-AUTH, WP3-AUTH |
| T8 | Agent-session files are agent-writable data that persists across the jobs of ONE analysis: stored per analysis id under the `runner` uid; only that analysis's files are copied into the per-job `CODEX_HOME`; `auth.json` is never part of the session files; deleted when the analysis is cancelled, superseded by a re-analysis, or past a retention period. A poisoned session affects only later jobs of the same PR analysis (same untrusted input). | WP3-RUN |
| T9 | **Accepted risk** (owner): v1 egress policy is allow-all with logging, so agent commands have open network access through the proxy; an injected PR could exfiltrate MemCan content, the shared MemCan token, the per-job Codex credential or the 1-hour read-only installation token. Mitigation in v1: the proxy logs every destination host (CONNECT host:port, bytes); TLS interception with full-URL logs and a host allowlist come later as configuration of the same Squid. | WP3-OPS (logging tests) |

## 6. Stage 0 — spikes (time-boxed, throwaway code)

Spike code in `/data/tmp/smart-review-spikes/<id>/`; at the end its reusable parts (Dockerfile, compose snippet,
recorded JSONL, sanitised GitHub responses, scripts) are archived with the evidence in
`/data/artifacts/smart-review/YYYY-MM-DD/spikes/<id>/`. Each spike writes `docs/spikes/S<id>-<name>.md`
(≤150 lines: question, runs, evidence, finding, decision, plan impact) and an ADR where it decides something.
Time box: one agent session (~3 h); overrun → stop and report what is known.

### 6.1 Owner session items (§1; all collected before any dispatch)
| # | Item |
|---|---|
| O1 | Push `main` to `lklimek/smart-review` (public; GitHub Actions enabled). |
| O2 | The existing GitHub App: App id, client id and secret, private key (.pem); callback URLs `https://review.klimek.ws/auth/github/callback` and a LAN development callback; permissions pull requests read/write, contents, checks, issues and metadata read; installed on `dashpay/dash-evo-tool` and `dashpay/platform`. |
| O3 | Codex device login: the owner runs `codex login --device-auth` in the coordinator-prepared spike container, writing to its `CODEX_HOME` volume. **Never mount or copy the host `~/.codex`** (token rotation would break the owner's own login). |
| O4 | One real login with the App (device or web flow): the user and refresh tokens are stored for spikes and the M3b smoke in `~/.config/smart-review-dev/github-user-token.json` (0600; never in git or `/data/artifacts`, which is published over HTTP). |
| O5 | MemCan URL and token (`MEMCAN_API_KEY`) as configuration values (MemCan moves to another host before deployment; only the values change). The owner edits MemCan's `TRAEFIK_IP_ALLOWLIST` himself: he adds the source address the egress-proxy container presents, which S6 determines (compose pins the proxy network's subnet so it stays stable); the coordinator puts that address under "For the owner" and `summon`s. Until it is added, agent jobs use the fake MemCan. |
| O6 | A reverse-proxy route `review.klimek.ws` → the server's HTTP port on the deployment host, with the owner's free certificate (it may answer 502 until the stack runs). |

Quota: no cap on the Pro plan; keep every real run purposeful.

**Test targets (fixed by the owner; public; the coordinator re-checks state before each use)**
| Id | PR | Used for |
|---|---|---|
| P-CAP | `dashpay/dash-evo-tool#1040` (merged, +1119/−21, 14 files, by lklimek) | S7, schema smoke, M2 real-Codex smoke, M3b session smoke, WP4-GUI |
| P-CAP2 | `dashpay/dash-evo-tool#1099` (merged, +917/−43, 16 files) | second S7 sample, WP4-REAL |
| P-REV | `dashpay/dash-evo-tool#1036` (open, by lklimek, +723/−178, 15 files); if merged or closed, another open PR by `lklimek` or `Claudius-Maginificent` | S3, M3b real-GitHub smoke |
| P-REF | `dashpay/dash-evo-tool#1039` (merged, +1216/−673, 68 files; mostly refactor) | WP4-REAL |
| P-LARGE | `dashpay/platform#5514` (merged, +1095/−755, 104 files) | WP4-REAL |
| P-XREPO | a `dashpay/platform` PR by those authors touching `platform-wallet` (e.g. #5311); dash-evo-tool depends on platform | WP4-REAL (related repositories) |

**Rule for real PRs**: only PENDING review comments are ever created; they are deleted at the end of the spike or
test; a review is never submitted or published (it would be visible to the dashpay team). A run that leaves a
pending comment behind is a defect to fix before anything else.

### 6.2 Spikes
| Id | Question | Done when | If it fails (coordinator decides, ADR) |
|---|---|---|---|
| S1a Codex basics | Unprivileged non-root container with subscription auth on the O3 volume; `codex exec --json --output-schema`: event stream shape, accepted schema subset (required/nullable, `additionalProperties`, enums, depth, `oneOf`), token refresh written back; exact model id for "Sol" and the reasoning-effort setting; hosts contacted | finding + 3 recorded JSONL runs (success, schema error, cancelled) + host list | Headless subscription auth impossible → no paid API; continue every stage on the fake provider; record the blocker, `summon` the owner. |
| S1b Codex sessions and isolation | `codex exec resume <id>` from a fresh work dir with a per-job `CODEX_HOME` populated from a copied session archive; the non-interactive equivalent of `/compact` and how to trigger it at job end; whether and how sub-agents work in `exec`; cancel via SIGTERM; Codex sandbox (bwrap/landlock) inside the container; ignoring repository config (`--ignore-user-config`, `--ignore-rules`, AGENTS.md handling); MCP client config (stdio server with env, streamable HTTP with bearer, `enabled_tools`); usage-limit error shape | finding doc + recorded resume/compaction runs | Resume or compaction unavailable non-interactively → every job is a new session with a context pack. No sub-agents → the prompt does not offer them. Sandbox fails → off, T1–T4 compensate. Repo config not ignorable → runner removes those files from the work dir before the run. |
| S2 claudius skills | How Codex 0.167 discovers skills from a git checkout of `lklimek/claudius` at a configurable ref; is `severity` actually used? | finding + run log | Inject needed skill files from the checkout into the prompt at job start. |
| S3 Pending review | GraphQL on P-REV with the O4 token (pending only, deleted afterwards): create pending review with first thread, add thread, multi-line, deleted-line (LEFT), edit, delete, find + reuse existing pending review, file-level comment with a link to a line outside the diff, list pending comments | finding + **sanitised recorded responses** (become fake-github fixtures) | REST pending-review endpoints; if file-level comments cannot be pending, post them as the review body. |
| S4 App tokens | User tokens (O4): expiry, refresh-token rotation, web-flow callback requirements. Installation tokens: App JWT from the private key, minting with `permissions` limited to read and `repositories` limited to PR repo + related repos, 1 h expiry; can such a token read public issues/PRs of a repository where the App is not installed? Installation status query | finding + sanitised recorded responses | Device flow for login. Related repo outside the installation → token without that repository; the agent reads it as public data if S4/S5 show that works, otherwise related work for it is skipped (ADR). |
| S5 GitHub MCP for the agent | Official `github-mcp-server` (pin version) in read-only mode with an S4 installation token: accepted? PRs, issues and search on `dashpay/dash-evo-tool`, `dashpay/platform` and a public repo without the App; started by Codex from the per-job config as uid `agent`; tool calls visible in Codex JSONL; it works through the explicit proxy | finding + recorded JSONL | Installation token rejected → server-side read-only lookup tool executing with the App installation token (ADR, "For the owner"); changes `JobSpec`, hence in the gate. |
| S6 Squid egress proxy | Squid (Ubuntu 26.04 `squid-openssl`, image as in `research/egress-proxy-squid-tested.Dockerfile`) as an **explicit** proxy, allow-all, no SSL-Bump, no CA, no cache, plus the SSRF guard of `research/egress-proxy.md` §3; runner on an internal network, proxy its only egress. Test: Codex, git, cargo, curl and `github-mcp-server` work through it; Codex passes the proxy env variables to the commands it runs; long streams (WebSocket/SSE, >30 min) survive; a host-name log (CONNECT host:port, bytes) is produced; private LAN unreachable except MemCan; the source address MemCan sees (for O5) | compose snippet + logs per test item + the address for O5 | A tool ignores the proxy env → set its own proxy option in the per-job config or the image (ADR). Codex cannot work through the proxy → record the blocker, continue on the fake provider, `summon`. Read-only root fs fails → writable root with tmpfs (ADR). |
| S7 LLM capability | Draft `plan` and `analyse_flow` schemas in the S1a subset; run `plan`, `pr_context`, then `analyse_flow` for 2–3 flows on P-CAP (P-CAP2 as second sample) through ONE resumed session, compacting after each job: schema validity, coverage of changed lines, plausibility of flows and flow graph, runtime, usage, context size per job before/after compaction, repairs needed | finding with numbers + outputs archived; go/no-go on the agent-output shape and on sessions | Shape not producible → simplify the schema (fewer nested levels, split `analyse_flow` into graph + steps) before the freeze. Context grows without bound → new session per job with a context pack. |

Dispatch (`claudius:developer-bilby`, opus): agent 1 = S1a → S1b → S2; agent 2 = S3 + S4; agent 3 = S5 + S6
(after S1a's container exists and S4 can mint tokens); agent 4 = S7 (after S1a, uses S1b's findings when
ready). One heavy job at a time (S5/S6 image builds included).

**Exit gate stage 0** (coordinator): S1a, S1b, S3, S4, S5, S7 decided (they shape contracts and `JobSpec`);
S2 and S6 may finish during stage 1 but before WP3-RUN / WP3-OPS start. Every fallback goes to "For the owner".

## 7. Stage 1 — contracts and skeleton

| WP | Goal | Owned paths | Agent / model | Size |
|---|---|---|---|---|
| WP1-S Skeleton | Workspace, all crates with `lib.rs`/`main.rs` stubs, all §5 dependencies declared, lints, rustfmt, toolchain, server module skeleton (`state.rs`, `config.rs` with all §5.2 fields, empty modules, router registration, feature `embed-ui`), `scripts/check-architecture.sh`, ADR-001 (§5.2) | root manifests and configs, `crates/*/Cargo.toml`, `crates/*/src/{lib,main}.rs`, `crates/server/src/{state,config}.rs`, `crates/server/src/api/mod.rs`, module stub files, `scripts/check-architecture.sh`, `docs/decisions/ADR-001-*` | developer-bilby, opus | ~400 |
| WP1-A Contracts | Everything in §5.1/§5.3 after WP1-S: contract types incl. sessions and agent credentials, generation + drift test, routes, traits, `analysis` public signatures (verify, coverage, ordering, moves, related repos), `db` repository API signatures (typed `NotImplemented` error, no `todo!()`), `0001_init.sql` (incl. tenant settings seed), `docs/contracts/{http-api,sse-events,runner-protocol,db-schema,agent-output}.md`, `scripts/schema-smoke.sh` | `crates/contracts/**`, trait and signature files in `analysis`, `db`, `server/src/jobs/executor.rs`, `runner/src/provider/mod.rs`, `crates/db/migrations/**`, `schemas/**`, `web/src/api/generated/**`, `docs/contracts/**` | developer-bilby + architecture skill, opus, plan gate | ~1,000 (types) + docs |
| WP1-T Threat model | `docs/contracts/threat-model.md` (one page): trust boundaries, assets, T1–T9 refined with S1b/S4/S5/S6 findings, compensating controls, accepted risks (T6, T8, T9) | `docs/contracts/threat-model.md` | security-engineer-smythe, opus | doc |

WP1-A inputs: brief §3, §5, §6; S1a/S1b/S3/S4/S5/S7 findings; mockup.js data shapes; §4–§5. From
`prior-art.md` §5 take the structure only and **drop**: `confidence`, `summary.overall_risk`,
`review_effort_minutes`, float `importance` (use an integer rank), `risk.likelihood/impact`, `noise` (→ Not
analysed, server-side), `bug_suspect` (→ `question`), `recommendation`, `tour_order`, `verified_by: llm_only`,
LLM `call_hierarchy`.

WP1-A acceptance: workspace builds; clippy clean and tests green for `-p contracts`; drift test green; schema
tests (closed enums, no floats, no confidence, `basis` + evidence rule); `check-architecture.sh` passes;
**schema smoke**: `scripts/schema-smoke.sh` runs one real `codex exec --output-schema` per job kind (S1a
container, P-CAP) and every final schema is accepted — log archived.

**Contract gate (coordinator)**: architecture review of contracts + threat model against brief and §4–§5, plus
the independent review of WP1-A and WP1-T (Codex Astra, xhigh). Approval is recorded in an ADR; nothing in
stage 2 starts before it. Then, in parallel:

| WP | Goal | Owned paths | Agent / model | Size |
|---|---|---|---|---|
| WP1-B Fixtures | Example PR `acme/node-client#482`, 5 flows / 17 steps (the 14 steps of mockup.js and its "5 flows, 14 steps" summary, plus 3 so that every changed line lies in a step, `fixtures/pr-482/README.md`; the mockup DOM shows 12 blocks): `repo/{base,head}/` trees + helper building a git repo; `github/` from the S3/S4 recorded responses adapted to #482; `agent/plan.json`, `pr_context.json`, `flow-1..5.json`; `codex/*.jsonl` from S1a/S1b adapted (incl. a resumed session); `api-seed/default/` only for the slice endpoints; `api-states/<state>.json` small JSON merge patches (RFC 7396) for `wrong`, `missed`, `noq`, `noprofile`, `resume` | `fixtures/pr-482/**`, `crates/contracts/tests/fixtures.rs` | developer-bilby, opus | data + ~200 |
| WP1-C CI | CI: fmt, clippy `-D warnings`, nextest, doctests, drift test, architecture check, wording check (web source strings only); web jobs no-op until `web/package.json` exists | `.github/workflows/ci.yml`, `scripts/check-wording.sh` | developer-bilby, opus | ~250 |

WP1-B acceptance: agent fixtures validate against `schemas/agent/*`; API seed and patched states deserialise
into contract types and re-serialise to the same JSON (round-trip catches unknown fields); fixture text passes
the wording rules. Fixture ownership afterwards: §4.3.

**Exit gate stage 1 (M1)**: contracts, threat model, fixtures and CI merged; schema smoke archived; CLAUDE.md
"Build and test" filled in.

## 8. Stage 2 — thin vertical slice

Goal: URL → analysis with fixture agent output → Tour renders flow 1 through real server, runner and browser.
Scope is deliberately thin: no pre-pass rules, no repair, no comments, no Ask, no session resume (each job
starts a session), no agent GitHub token; one happy path. All WPs run in parallel; Rust WPs queue their cargo
runs. All agents: `claudius:developer-bilby`, opus.

| WP | Goal | Owned paths | Depends | Size |
|---|---|---|---|---|
| WP2-JOBS | `QueueExecutor`, job queue (claim with lease, one running job per session, events, result idempotent per attempt), job tokens, runner protocol endpoints, SSE progress, last LLM call outcome recorded from results | `server/src/jobs/**`, `server/src/api/{runner,events}.rs`, `db/src/repo/{jobs,analyses}.rs` | M1 | ~900 |
| WP2-AUTH | GitHub App web login against configurable base URL, sign-in only for logins in the tenant settings, session cookie (Secure behind trusted proxy), AES-GCM token storage | `server/src/{auth,secrets}/**`, `server/src/api/auth.rs`, `db/src/repo/{users,sessions,tokens,tenants}.rs` | M1 | ~700 |
| WP2-INGEST | PR resolve + ingest (REST metadata), bare mirror fetch from the configurable clone base, hunks (`analysis::diff`), pipeline skeleton (`pr_context` + `plan` → `analyse_flow` → schema validation → persist) via `JobExecutor`, tour and lines endpoints, `embed-ui` serving, `select_model` (plain function: Sol, high), fake-github contract test | `server/src/{github,pipeline,frontend}/**`, `server/src/model.rs`, `server/src/api/{pr,analyses,tour,lines}.rs`, `analysis/src/diff.rs`, `db/src/repo/{repos,pull_requests,documents}.rs`, `server/tests/**` | M1 | ~1,000 |
| WP2-RUN | Runner loop, protocol client, `execute()` (mirror cache fetch, fresh work dir checkout, provider run, event streaming, result, cleanup), fake provider (feature), **minimal Codex provider `run`** (exec + schema + JSONL parse; no resume/compaction/cancel/limits) | `crates/runner/**` | M1, S1a | ~900 |
| WP2-FAKE | `tools/fake-github`: OAuth, `/user`, PR/commits/files REST from `fixtures/pr-482/github` | `tools/fake-github/**` | M1 | ~400 |
| WP2-WEB | Vite + React + TS + Tailwind + TanStack Query + react-router; mockup design tokens (light and dark) and structure ported from `docs/mockups/mockup.css` into the Tailwind theme; `api/client.ts`; msw handlers from `api-seed`; ESLint rule A7; Tour minimal: left menu, PR context block, flow introduction, step block with plain code view (neutral gutter marks, one highlight), Previous/Next | `web/**` except `web/src/api/generated` | M1 | ~1,000 |

Acceptance (named tests):
- WP2-JOBS: claim/lease/expiry/re-queue; a second job of the same session is not claimed while one runs; wrong
  or expired token rejected; result posted twice → stored once; SSE emits progress for a scripted sequence.
- WP2-AUTH: login against wiremock GitHub; `allowlist_rejects_unlisted_login` (tenant settings); token row is
  ciphertext; `secure_cookie_behind_trusted_proxy`; logout clears session.
- WP2-INGEST: `slice_pipeline` (server + scripted executor returning fixture agent output) → analysis
  `finished`; its tour JSON is written as the server golden `fixtures/pr-482/api/default/tour.json` with
  `UPDATE_GOLDEN=1` (first creation reviewed by the coordinator against `api-seed`, then `api-seed` is deleted);
  lines endpoint caps the range; fake-github contract test; `select_model_returns_sol_high`.
- WP2-RUN: fake provider reproduces fixture events/result; work dir removed on success/failure; empty cache
  works (A4); recorded S1a JSONL parsed by the Codex provider.
- WP2-WEB: vitest per block; Playwright (msw): PR context → flow 1 introduction → step 1 via `j`.

**M2 merge point**: merge all; dispatch **WP2-E2E** (developer-bilby, opus, ~250; owns `e2e/**`,
`deploy/dev/**`): full-stack Playwright — server (`embed-ui`) + runner (fake provider) + fake GitHub + fixture
remote: sign in, paste URL, progress reaches finished, Open review, flow 1 step 1 shows the fixture code. Then:
- **Real-Codex smoke** (WP2-E2E agent, coordinator grants the slot): the slice with the Codex provider on
  P-CAP; analysis finishes or fails visibly; outputs archived; prompt or schema problems become stage-3 inputs.
- **Visual conformance check 1** (§12.5): screenshots of the slice against the mockups.
- Merge-point review; push `feat/v1`.

## 9. Stage 3 — breadth

All WPs start after M2 unless "Depends" says otherwise. Run at most three Rust agents at once; UI and ops
agents fill the gaps. Every WP's acceptance items are named tests; trust tests per §12.4. All agents:
`claudius:developer-bilby`, opus. "Sec" = security-sensitive (full review with security pass, Astra xhigh).

| WP | Goal (brief refs) | Owned paths | Depends | Sec | Size |
|---|---|---|---|---|---|
| WP3-AUTH | Refresh-token rotation, logout everywhere, CSRF, Secure/trusted-proxy settings, installation status (§2 Users, §5 Security) | `server/src/{auth,secrets}/**`, `server/src/api/auth.rs`, `db/src/repo/{users,sessions,tokens,tenants}.rs` | S4 | yes | ~700 |
| WP3-APPTOKEN | App JWT from the private key file; per-job installation token: read-only permissions, `repositories` = PR repository only (ADR-005, threat model T3a), 1 h; placed in `JobSpec`; key and tokens never logged (§5 GitHub access for the agent, T3) | `server/src/github/app_token.rs`, its tests | S4, S5 | yes | ~400 |
| WP3-REVIEW | Threads + checks ingest; pending review via GraphQL (create, reuse, add, edit, delete, count, list) per S3; comments API with idempotency key; a comment on a line outside the diff becomes a file-level comment whose body links to the line; feedback `missed_by_llm` when the comment is on a **changed line without annotation** (§3 Feedback, §5 Comments) | `server/src/github/{threads,checks,review}.rs`, `server/src/api/comments.rs`, `db/src/repo/{comments,threads}.rs` | S3 | yes | ~900 |
| WP3-PREPASS | `generic` plugin (path globs, line regexes, generated headers; `call_hierarchy → None`), moved blocks (`--color-moved` + token-normalised hashes + `similar`), whitespace-only, counters, related repos from Cargo files, same org (§5 Pipeline 2, Tech plugin, §7) | `analysis/src/{tech,moves,counters,related}/**` | — | — | ~900 |
| WP3-VERIFY | Verification (schema, ranges, coverage → Unexplained, move claims, graph consistency, fact-needs-evidence, banned phrases in LLM text), repair feedback, tour ordering (behaviour by importance → LLM-classified refactors → LLM-classified tests → quiet rows; Unexplained first among quiet rows) (§1, §5 Pipeline 6, §4.3) | `analysis/src/{verify,coverage,order}/**` | — | — | ~800 |
| WP3-RUN | Codex provider complete per S1b/S2/S5: `resume` and end-of-job compaction, session store per analysis (copy in/out, deletion, retention), typed `SessionUnavailable`, per-job `CODEX_HOME` with `github-mcp-server` (read-only, installation token) and MemCan MCP, repo-config neutralisation, sub-agent setting per S1b, proxy env per research table, normalised events, typed usage-limit error with `resumes_at`, `cancel`, claudius refresh (interval, ref), T1–T6, T8 | `crates/runner/**` | S1b, S2, S5, WP1-T | yes | ~1,000 |
| WP3-PIPE | Orchestration: pre-pass + verify integration, one agent session per analysis (session ref on every job, `started_after_loss` → context pack from stored results, new session on re-analysis), installation token per job (via WP3-APPTOKEN), huge-PR plan, `pr_context`, one `analyse_flow` per flow, ≤2 repairs, progress `k of n`, pause/resume, cancel (partial results and session discarded), retry, failure before any flow (plain description + Retry), resume of an existing analysis, LLM status of threads located in a flow's steps (others: none); context-pack builder shared with Ask (§5 Agent sessions, Pipeline, §2 Resume, §4.2) | `server/src/pipeline/**`, `server/src/api/analyses.rs` | PREPASS, VERIFY, APPTOKEN merged first | — | ~1,000 |
| WP3-PROMPTS | Prompt per job kind: review lenses, project profile union (file wins), LLM-text language, trust wording, "read ranges, never whole large files", generated files never given; no numeric likelihoods; test code inside production files classified `test`; related work via `github-mcp-server`; sub-agents permitted for large PRs at the agent's discretion (if S1b supports them) (§5 Agent context, Agent sessions, §3, §7) | `prompts/**`, `server/src/prompts.rs` | S5, S7 | — | ~500 |
| WP3-ASK | Ask job on `JobExecutor` resuming the analysis's session (context pack on loss); questions API; answer streamed via SSE and stored as `Question`; `ask` claimed before the next batch job, no pre-emption (§5 Ask) | `server/src/ask/**`, `server/src/api/questions.rs`, `server/src/jobs/claim.rs`, `db/src/repo/questions.rs` | WP3-PIPE (context-pack builder) | — | ~500 |
| WP3-MEMCAN | MemCan rmcp client with bearer token (feedback; calibration examples for overrated/underrated; `project` = analysed repository short name), feedback API (§5 Calibration memory) | `server/src/memcan/**`, `server/src/api/feedback.rs`, `db/src/repo/feedback.rs` | — | — | ~500 |
| WP3-STATE | Review-state API (ticks, last viewed block), settings API (language, LLM status from the most recent LLM call, per-repo read-only rules, related repos, profile source), `diff` endpoint (§2 Progress, §4.4) | `server/src/api/{review_state,settings,diff}.rs`, `db/src/repo/{review_state,settings}.rs` | — | — | ~600 |
| WP3-CODEVIEW | Line renderer: Shiki on displayed lines only; neutral mark on every changed line; LLM bar solid/dashed/dotted only on annotated ranges; `≠` moved; markers (remark, deleted site, thread, own comment); hover was → is only; comment-line highlight; inline old code (§4.3 Code view) | `web/src/components/code-view/**` | — | — | ~800 |
| WP3-DIAGRAM | Flow diagram from graph data (layout lib chosen and verified: `elkjs` 0.12.1 or `@dagrejs/dagre` 3.1.3); kind by colour AND border/marker; branch labels; current step; click → step (§4.3 Centre) | `web/src/components/flow-diagram/**` | — | — | ~500 |
| WP3-BLOCKS | Navigation list (labels for LLM-classified refactors and tests), PR context, flow introduction, step block (four facts + code slot), quiet-row blocks (moves, Not analysed, Unexplained, pending-review list), badge + tooltip primitives, help dialog (legend, shortcuts) (§4.3) | `web/src/components/{nav,pr-context,flow-intro,step,quiet,badge,tooltip,help}/**` | — | — | ~1,000 |
| WP3-INSPECTOR | Inspector (never empty; current block or selected annotation; Back to step) with 8 collapsible sections (state in localStorage; call hierarchy "n/a — needs a language plugin"); dock: Ask, feedback buttons, comment composer on any line (outside the diff: note "posted as a file comment") (§4.3 Inspector) | `web/src/components/{inspector,ask,feedback,comment}/**` | — | — | ~1,000 |
| WP3-SCREENS | Home (sign-in, URL, resume, head-moved banner, errors incl. login not allowed, "Analysis cancelled — analyze again"), Analysis progress (SSE; planning with indeterminate bar "Planning flows…", running, paused, failed incl. before any flow, finished), Settings with LLM status of the last call (§4.1, §4.2, §4.4) | `web/src/screens/{home,progress,settings}/**` | — | — | ~700 |
| WP3-TOUR | Tour assembly: top bar, 3 resizable/hideable/full-screen columns remembered, block sequence and flow order, keyboard map, ticks + "flows reviewed k / n", last viewed block, light/dark theme, six fixture states (§4.3) | `web/src/screens/tour/**`, `web/src/app/**` | BLOCKS, INSPECTOR, CODEVIEW, DIAGRAM | — | ~900 |
| WP3-OPS | Dockerfiles (server: web build + Rust with `embed-ui`, `CARGO_BUILD_JOBS=2`; runner: pinned Codex CLI and `github-mcp-server`, git, ripgrep, two uids, no Docker socket); `deploy/egress-proxy/` from the research Dockerfile with a v1 config (explicit proxy, allow-all, host-name log, SSRF guard; no SSL-Bump, no CA, no cache); compose (server, runner, egress-proxy, named volumes incl. session store, internal runner network, pinned proxy subnet, healthchecks, no runtime containers, plain services for Portainer); `.env.example` with all §5.2 variables; proxy log retention and the later interception/allowlist step (reference to the research) in the runbook; CI image builds and web jobs; release builds without fakes (§5 Packaging, T1–T4, T8, T9) | `deploy/**`, `.github/**`, `.dockerignore`, `docs/runbook.md` | S6, WP1-T | yes | ~900 |
| WP3-E2E | Fake MemCan; full-stack Playwright extended: comments (create/reuse/edit/delete, outside the diff), Ask, feedback, resume, head moved, cancel, the three hard cases, rendered-text wording in all states; compose-based run in CI | `e2e/**`, `tools/fake-memcan/**` | M3a + M3b | — | ~700 |

Key acceptance tests (in addition to the brief items in §11):
- WP3-AUTH: refresh rotation persisted; CSRF rejection; no token in logs (log-capture test).
- WP3-APPTOKEN (wiremock GitHub): `installation_token_read_only_permissions`, `installation_token_repositories_scoped`,
  `installation_token_expiry_one_hour`, private key and tokens absent from logs and errors.
- WP3-REVIEW: second comment reuses the pending review; double submit with one key → one comment (G-DOUBLE);
  edit/delete reach fake GitHub; `outside_diff_comment_is_file_level`; `missed_by_llm` only for changed lines
  without annotation.
- WP3-PREPASS/VERIFY: proptest coverage invariant (every changed line in exactly one flow, Not analysed item or
  Unexplained); identical vs differing moves; generated file never in agent input; dangling edges, unmapped
  nodes, out-of-range lines and facts without evidence rejected; quiet rows never above a rated flow.
- WP3-RUN: golden JSONL incl. unknown events and a resumed session; compaction issued at job end; usage limit →
  typed error; cancel kills and cleans; `session_files_only_own_analysis`; `session_files_exclude_auth`; lost
  session → `SessionUnavailable`; session files deleted on cancel/supersede/retention; generated config has
  only `github-mcp-server` (read-only flag, installation token) and MemCan; agent uid cannot read runner
  secret, master `CODEX_HOME`, mirror or other analyses' sessions (container test); agent env has no job token.
- WP3-PIPE: scripted-executor tests for happy path, `jobs_carry_analysis_session`, `jobs_carry_installation_token`,
  `lost_session_uses_context_pack`, `reanalysis_starts_new_session`, repair success, repair exhausted → failed
  flow + Retry, usage limit → paused → resumed, cancel, failure before flows, head moved → new analysis without
  ticks, resume keeps ticks/answers/last block. WP3-PROMPTS: insta snapshots per job kind and language.
- WP3-MEMCAN (fake MemCan): bearer token sent; calibration example with `project` = repo short name.
- WP3-STATE: `llm_status_reflects_last_call` (ok, error with plain message, time).
- UI WPs: vitest per component (props in, callbacks out, no fetch); keyboard map; layout persistence; both themes.
- WP3-OPS: compose with fake provider passes full-stack Playwright; public host allowed and logged (CONNECT
  host:port, bytes); private LAN destination blocked and logged; MemCan reachable; non-root.

**Merge points** (coordinator gates; each ends with a merge-resolution review and a push)
- **M3a (UI)**: BLOCKS, INSPECTOR, CODEVIEW, DIAGRAM, SCREENS, TOUR. Playwright (msw) for all six states;
  **visual conformance check 2** (§12.5) on all six states and all screens.
- **M3b (backend)**: PREPASS, VERIFY and APPTOKEN before PIPE; PIPE before ASK; AUTH, REVIEW, RUN, PROMPTS,
  MEMCAN, STATE independent. **Real-GitHub smoke** with the O4 token on P-REV (pending only, deleted
  afterwards): pending review create/reuse/edit/delete, file-level comment, threads, checks (ignored test reading
  the token file; never in CI). **Installation-token smoke**: mint, then `github-mcp-server` reads P-CAP and a
  public repo without the App. **Real-Codex session smoke** on P-CAP: one analysis through one resumed
  session, then one Ask.
- **M3c**: OPS and E2E.

## 10. Stage 4 — hardening, deployment, GUI tests

| WP | Goal | Agent / model | Size |
|---|---|---|---|
| WP4-SEC | Audit of the merged tree against WP1-T: auth/session/CSRF, token storage, App key and installation tokens, runner uids, proxy and its logs, session store (T8), accepted risks T6/T9 still as described, prompt-injection paths, secrets in logs, SSE auth, path traversal in lines/diff; fixes go to the owning WP's agent | security-engineer-smythe, opus | review |
| WP4-REAL | Codex on P-CAP2, P-REF (mostly refactor), P-LARGE (104 files) and P-XREPO (related repository): runtime, usage, validity, repair rate, session size, refactor labelling, related-repo context; prompt tuning in `prompts/` | developer-bilby, opus | ~300 |
| WP4-PERF | Synthetic large PR (10k-line diff, 15k-line file, 140k-line generated file): range limits, bounded event log, SQLite WAL under SSE + runner posts | developer-bilby, opus | ~300 |
| WP4-DOCS | README, runbook (GitHub App, Codex device login into the runner volume, `SR_SECRET_KEY`, proxy log retention, MemCan allowlist entry, later TLS interception and allowlist, backups, upgrade), CLAUDE.md commands | technical-writer-trillian, opus | docs |
| WP4-DEPLOY | Deploy the compose stack on the deployment host behind the owner's reverse proxy (O6) with `SR_PUBLIC_URL=https://review.klimek.ws`; check login with the real App, cookies, SSE through the proxy | developer-bilby, opus (coordinator grants the slot) | config |
| WP4-GUI | Exploratory + scripted GUI testing of the REAL deployed stack in a real browser (`playwright-cli` skill): the whole path on a real analysis of P-CAP (sign in, analyse, progress, Tour, Ask, feedback, pending comment created and deleted), comparison with `docs/mockups/` and the brief, keyboard map, light and dark theme, wide (21:9) and narrow viewports, error states; findings go back to implementers; report archived | qa-engineer-marvin, opus | review |

Exit (coordinator): acceptance run against §13; WP4-GUI findings fixed or recorded; one **full**
`claudius:review-changes` of `feat/v1` (Codex Astra, xhigh) with all findings resolved or recorded;
`docs/v1-handover.md` written from the "For the owner" section (ADRs, deviations, accepted fallbacks and
risks, owner-only follow-ups such as Portainer import on the owner's host); `feat/v1` pushed. No pull request
is opened; the owner decides about merging after deployment.

## 11. Traceability (brief v1 → work package → test)

| Brief item | WP | Test (name or kind) |
|---|---|---|
| §2 Four screens; loading/empty/error states | SCREENS, TOUR, GUI | Playwright per screen and state; WP4-GUI |
| §2 Input: PR URL, public repos only; errors | INGEST, SCREENS | `resolve_rejects_non_pr_url`, `resolve_rejects_private_repo`, Home error Playwright |
| §2 GitHub login; single tenant `default`; allowed logins as tenant setting (seeded `lklimek`) | AUTH (2+3), WP1-A | login e2e; `tenant_default_seeded`; `allowlist_rejects_unlisted_login` |
| §2 Review opens only when finished | PIPE, SCREENS | Playwright: no Open review before finished |
| §2/§3 PR context (problem, related work, fit questions, CI, counters, profile status) | PIPE, PROMPTS, PREPASS, BLOCKS | pipeline fixture test; PR-context component test |
| §2 GitHub threads at steps with LLM status (from `analyse_flow`; others none); CI status | REVIEW, PIPE, INSPECTOR | thread placement and status tests; top-bar CI test |
| §2/§5 Ask (resumes the analysis session, pinned LLM answer, priority at claim) | ASK, RUN, INSPECTOR, E2E | `ask_resumes_analysis_session`, `ask_claimed_before_batch`, `ask_answer_stored_and_streamed`; Playwright Ask |
| §5 Agent sessions: one per analysis, compaction, sub-agents optional, loss fallback, new on re-analysis | RUN, PIPE, PROMPTS | WP3-RUN and WP3-PIPE session tests; M3b session smoke |
| §2 Ticks, flow reviewed, "flows reviewed k / n", last viewed block persisted | STATE, TOUR | review-state API tests; Playwright reload keeps position |
| §2/§5 Comments into GitHub pending review (create/reuse/edit/delete, count, list); outside the diff as file-level comment with line link | REVIEW, INSPECTOR, E2E | WP3-REVIEW tests; Playwright comments; M3b real-GitHub smoke |
| §2/§3/§5 Feedback values + note; `missed by the LLM`; MemCan with scope = repo short name; calibration | MEMCAN, REVIEW, INSPECTOR, E2E | feedback API tests; hard cases 1–2 |
| §2/§4.1 Resume; head moved banner; Re-analyze; open old analysis | PIPE, SCREENS, E2E | `head_moved_new_analysis_without_ticks`; Playwright banner |
| §2/§5 Project profile union, file wins; "no project profile" | PROMPTS, PIPE | `profile_union_file_wins`; `noprofile` state |
| §2/§5 Related repos from Cargo files, same org | PREPASS, APPTOKEN | `related_repos_same_org_only`; token repository scope test |
| §2 UI English; LLM texts in configured language | PROMPTS, STATE | prompt snapshot per language; settings test |
| §2/§5 `generic` plugin; path globs, line regexes; call hierarchy n/a | PREPASS, INSPECTOR | `generic_call_hierarchy_none`; "n/a" component test |
| §2 Huge PRs split into many runs | PIPE, PERF | `huge_pr_plan_splits_by_directory`; WP4-PERF |
| §2/§5 Codex CLI, provider trait, usage-limit pause/resume; model selection (plain function, Sol, high) | RUN, PIPE, SCREENS, INGEST | usage-limit JSONL test; paused state Playwright; `select_model_returns_sol_high` |
| §2/§5 Agent access: read-only checkout, MemCan (read and write), claudius, `github-mcp-server` read-only with a 1-hour installation token, related repos | RUN, APPTOKEN, OPS | T1–T9 tests; M3b installation-token smoke |
| §2 Deployment: compose, Portainer-compatible; `SR_PUBLIC_URL`; TLS at the reverse proxy, HTTP behind it | OPS, AUTH, DEPLOY | compose e2e; `secure_cookie_behind_trusted_proxy`; WP4-DEPLOY |
| §5 Egress proxy: Squid as explicit proxy, allow-all, host-name log, private LAN denied except MemCan | OPS | S6 logs; WP3-OPS proxy tests |
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
| §4 Full width; light/dark theme; PR number link; tooltip on every label; legend in `?`; top bar | TOUR, BLOCKS, GUI | Playwright tooltip sweep; top-bar test; WP4-GUI themes and viewports |
| §4.1/§4.2 Progress: planning, running, paused, failed (incl. before any flow), finished; cancel discards partial results; Home cancelled message | SCREENS, PIPE, JOBS | Playwright per state; `cancel_discards_partial_results` |
| §4.3 Sequence and flow order; Previous/Next; keyboard map | TOUR, VERIFY, GUI | Playwright keyboard; ordering test |
| §4.3 Columns resizable, hide, full screen, remembered | TOUR | Playwright layout persistence |
| §4.3 Diagram hidden by default; one block at a time | TOUR, DIAGRAM | Playwright |
| §4.3 Inspector never empty; sections order; collapse remembered | INSPECTOR | Playwright per block type |
| §4.3 Show old code; classic diff | STATE (`diff`), CODEVIEW | diff endpoint test; `d` key Playwright |
| §4.3 Three hard cases; no profile; interrupted review | E2E, TOUR | §12.4 |
| §4.4 Settings: LLM status of the last call, language, App status, per-repo read-only; first Codex login via runbook | STATE, JOBS, AUTH, SCREENS, DOCS | `llm_status_reflects_last_call`; Playwright |
| §5 `JobSpec`/`JobExecutor`/`execute`; job kinds; caches and session store optional | WP1-A, JOBS, RUN | A3/A4 tests |
| §5 Pipeline steps 1–6, up to 2 repairs | INGEST, PIPE, VERIFY | WP3-PIPE tests |
| §5 Multi-tenancy columns, seeded tenant and tenant settings | WP1-A | migration test |
| §5 Secrets encrypted; App key server-only; cookie sessions; CSRF; trusted proxy | AUTH (2+3), APPTOKEN | WP2/3-AUTH and APPTOKEN tests |
| §5 Frontend: self-contained components | all UI WPs | ESLint A7 |
| §7 Large files: range API, displayed-lines Shiki, generated never given, ranges in prompts | INGEST, CODEVIEW, PREPASS, PROMPTS, PERF | cap test; WP4-PERF |

## 12. Verification strategy

### 12.1 Per stage
| Stage | Unit | Contract | End-to-end / real |
|---|---|---|---|
| 0 | — | — | real Codex (S1a, S1b, S7), real GitHub (S3, S4, S5), Squid (S6) |
| 1 | contract serde, schema rules | drift test, fixture round-trip | schema smoke per job kind (real Codex) |
| 2 | per WP | server golden tour; fake-github contract | full-stack slice Playwright; real-Codex slice smoke; visual check 1 |
| 3 | per WP | golden Codex JSONL; route ↔ msw handler test | Playwright six states (msw) + full-stack; real-GitHub, installation-token and real-Codex session smokes; visual check 2 |
| 4 | — | real outputs validate | WP4-REAL, WP4-PERF, WP4-DEPLOY, WP4-GUI, acceptance run, full review |

### 12.2 Independent review
Every WP with ≥50 code lines gets an independent review per `claudius:review-changes`, reviewer = Codex Astra
(`--model gpt-6-astra`, dispatched through `codex-companion.mjs task` per `claudius:delegate` § External Codex
launcher; never `codex:codex-rescue`). Effort xhigh for contracts + threat model, security-sensitive WPs
(Sec column; WP2-AUTH), merge points and the final full review; high for the rest. If Astra is unavailable:
Codex Sol at high effort, noted under "For the owner". Findings are fixed by the same implementing agent; merge
classes per `claudius:severity`.

### 12.3 Security
Design before the freeze (WP1-T, §5.4); full reviews with security pass for WP2-AUTH, WP3-AUTH,
WP3-APPTOKEN, WP3-REVIEW, WP3-RUN, WP3-OPS; audit at the end (WP4-SEC).

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

### 12.5 Visual conformance (delegated, no owner wait)
At M2 and M3a, `claudius:ux-designer-diziet` (opus, with a browser via the `playwright-cli` skill) compares
Playwright screenshots of every screen and fixture state, light and dark, with `docs/mockups/` and brief §4
and reports deviations (layout, wording, states, tooltips). The coordinator decides per deviation: fix (back
to the owning UI WP), accept (ADR + "For the owner"), or brief wins over mockup. Screenshots and the report are
published under `/data/artifacts/smart-review/YYYY-MM-DD/screens/` for the owner's later review.

### 12.6 GUI tests of the real stack (WP4-GUI)
Mandatory before the exit of stage 4; scope in §10. Report and screenshots archived under
`/data/artifacts/smart-review/YYYY-MM-DD/gui-tests/`; every finding is fixed by the owning WP's agent or recorded
with a reason under "For the owner".

## 13. Definition of done for v1

- All rows of §11 have a passing test; the six mockup states pass in Playwright; visual conformance deviations
  fixed or recorded; WP4-GUI has no open blocking finding.
- The stack runs at `https://review.klimek.ws`; a real public PR (P-CAP) analysed end to end with Codex Pro
  through one agent session; Ask resumes it; pause/resume, cancel, retry, resume, re-analyze work.
- Comments land in the user's pending review (create, reuse, edit, delete, outside-diff file-level); feedback
  stored locally and in MemCan; Ask answers pinned and labelled.
- A1–A12 checks green in CI; WP4-SEC and the final full review have no open blocking finding; drift test green.
- The runbook deploys the stack from scratch; secrets never reach the browser or logs; `docs/v1-handover.md`
  lists everything in "For the owner"; `feat/v1` pushed.

**Not in v1** (brief §2 Later, Non-goals): call hierarchy content (trait method and API field exist,
`generic` returns `None`; tree-sitter-tags language plugins later, no rust-analyzer); Rust and Go tech plugins
(until then test code inside production files is LLM-classified and shown as its own flow); Claude provider;
private repositories; per-job containers (`ContainerExecutor`, one-shot runner entry, session archive shipped
with the job); TLS interception in the proxy (full-URL logs, splice list for `chatgpt.com`/`openai.com`, own CA
in the runner image, caching) and the host allowlist — later configuration of the same Squid per
`research/egress-proxy.md`; per-client MemCan tokens; built-in TLS/ACME;
versioning, changelog and schema migrations (before deployment); Groups workspace, overview, files tree;
partial review while running; re-review with carried ticks; recent-reviews list; dashboard; LLM auto-comment;
PR-description-vs-code check; static-analysis actions; GitHub "Viewed" sync; custom "not analysed" rules;
tenant UI, real tenants, billing; automatic approve/reject; local comment drafts; PR-level risk or size
statistics; numeric confidence.

## 14. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Codex cannot produce the agent-output shape reliably | S7 before the freeze; schema smoke at M1; real slice smoke at M2; WP4-REAL |
| Session resume/compaction not available in `exec`, or context grows | S1b/S7 decide; fallback: new session per job with a context pack; session size measured in WP4-REAL |
| Lost or corrupt session | Treated like a cache miss: context pack from stored results (`started_after_loss`) |
| `github-mcp-server` rejects installation tokens, or related repos lie outside the installation | S4/S5 before the freeze; fallbacks in §6.2 |
| Codex or a tool breaks through the proxy (proxy env ignored, long streams) | v1 only tunnels (CONNECT, no interception); S6 tests each tool and long streams; long `read_timeout` |
| Codex CLI format changes | Pinned CLI version; golden JSONL tests; unknown events tolerated |
| Token rotation with concurrency | One job per credential; `runner` uid is the single `auth.json` writer (T1) |
| Prompt injection from PR content | T1–T9; read-only checkout; read-only GitHub token; WP4-SEC |
| Exfiltration through open egress; poisoned memories | Accepted by the owner (T6, T9); host-name logs; interception and allowlist later as configuration |
| Agents drift from trust wording | Rendered-text and agent-output checks; closed enums; A9 |
| Contract churn during parallel work | Coordinator contract gate; CCR; drift test; pre-declared state/config/deps |
| Fake GitHub diverges from github.com | Fixtures recorded in S3/S4; real-GitHub and installation-token smokes at M3b |
| UI deviates from the mockups | Ported mockup tokens; delegated visual checks at M2 and M3a; WP4-GUI |
| Owner unavailable during implementation | Everything collected in the owner session; coordinator authority (§1); blockers recorded, `summon`, other work continues |
| Codex Astra unavailable for reviews | Codex Sol at high effort; noted under "For the owner" |
| Host OOM | One heavy job incl. Docker builds; web worker caps; ≤3 Rust agents at once |
| Huge PRs / files; DB and session-store growth | Range-capped API; generated files excluded; bounded event log; session retention; WP4-PERF |
| SQLite contention | WAL, one write pool in `db`, short transactions; WP4-PERF |
| ToS of subscription use | Single tenant; sign-in only for `lklimek`; `api_key` credential type kept |
| Squid security advisories | Rebuild the proxy image on every Ubuntu squid update (runbook; research §5 item 7) |
| TS 7 / Vite 8 / msw 3 churn | Verified in WP2-WEB; fallback to penny's versions (ADR) |

**Brief §8 open points**: pending-review API and file-level pending comments → S3 → WP3-REVIEW; Codex sandbox
in container → S1b → WP3-RUN, WP3-OPS; neutralising repo config → Codex: S1b → WP3-RUN, Claude: with the
Claude provider (not v1); Codex resume, compaction and sub-agents in `exec` → S1b, S7 → WP3-RUN;
`github-mcp-server` with installation tokens → S4, S5 → WP3-APPTOKEN, WP3-RUN; Portainer and containers created
outside the stack → not v1.

## 15. Agents and models

| Work | Agent type | Model / effort |
|---|---|---|
| All implementation WPs (spikes, skeleton, contracts, fixtures, CI, server, runner, analysis, web, ops, e2e, perf, deploy) | `claudius:developer-bilby` | Opus |
| Contract and CCR reviews | coordinator with `claudius:architecture`, or `claudius:architect-nagatha` | Opus |
| Threat model, security audit | `claudius:security-engineer-smythe` | Opus |
| Visual conformance at M2 and M3a | `claudius:ux-designer-diziet` | Opus |
| GUI tests of the real stack | `claudius:qa-engineer-marvin` (with `playwright-cli`) | Opus |
| Docs | `claudius:technical-writer-trillian` | Opus |
| Independent reviews | Codex Astra via `codex-companion.mjs task` (`--model gpt-6-astra`) | xhigh: contracts, threat model, Sec WPs and WP2-AUTH, merge points, final full review; high: the rest. Fallback Codex Sol, high |

Every brief: §2 block, goal, owned paths, inputs (paths), acceptance tests, worktree path + SHA, MemCan prior
knowledge, skills (`claudius:coding-best-practices` always; `claudius:security-best-practices` for security
WPs), reporting channel. Brief WHAT, not HOW. WPs over ~1,000 lines are split before dispatch.

## 16. Owner decisions (2026-11-16)

| Topic | Decision | Shaped |
|---|---|---|
| GitHub auth | GitHub App user tokens for the user's actions via the owner's existing App, installed on `dashpay/dash-evo-tool` and `dashpay/platform`; S4 verifies behaviour | §6.1 O2/O4, S4, WP3-AUTH |
| GitHub access for the agent | Server mints a read-only installation token per job (pull requests, issues, contents, metadata; PR repository only, ADR-005; 1 h); the runner starts the official `github-mcp-server` read-only with it; no server-side lookup tool; App private key is a server secret | §5.3, §5.4 T3, S4, S5, WP3-APPTOKEN, WP3-RUN |
| Egress proxy | Squid, chosen because it can later intercept TLS and cache. v1: explicit proxy, allow-all, log of host names (CONNECT host:port, bytes); no TLS interception, no CA, no cache. Later, as configuration of the same Squid (`research/egress-proxy.md`): interception with full-URL logs, splice list (`chatgpt.com`, `openai.com`), CA in the runner image, cache, host allowlist. Open egress is an accepted risk | §5.4 T4/T9, S6, WP3-OPS, §13 |
| Repository | `lklimek/smart-review` public; GitHub Actions enabled; the owner pushes `main` | §6.1 O1, WP1-C |
| Codex model | Sol at reasoning effort high for every job kind; `select_model(kind, context)` is a plain Rust function (no table, no setting), later a real decision function; exact id verified in S1a | §5.2, S1a, WP2-INGEST |
| Agent sessions | One Codex session per PR analysis; batch and Ask jobs resume it; compaction at the end of every job; sub-agents at the agent's discretion for large PRs; loss → new session with a context pack; re-analysis → new session; if `exec` lacks resume or compaction → new session per job with a context pack | §5.3, §5.4 T8, S1b, S7, WP3-RUN, WP3-PIPE, WP3-ASK, WP3-PROMPTS |
| MemCan | Traefik entrypoint (port 8190) with IP allowlist + shared bearer token; `SR_MEMCAN_URL`/`SR_MEMCAN_TOKEN` as pure configuration (MemCan moves before deployment); the token reaches the agent through the per-job config; full read and write for the agent (accepted risk); server writes feedback and calibration; scope = analysed repository short name; the owner adds the proxy's source address (from S6) to the allowlist himself | §5.2, §5.4 T6, §6.1 O5, WP3-MEMCAN, WP3-RUN |
| Deployment and URL | `SR_PUBLIC_URL` is configuration; owner's instance `https://review.klimek.ws` (callback `…/auth/github/callback`); TLS at the owner's reverse proxy; server HTTP behind it with trusted-proxy setting and Secure cookies; no built-in TLS/ACME | §5.2, §6.1 O2/O6, WP4-DEPLOY |
| Quota | No cap on the Pro plan; real runs purposeful | §6.1 |
| Test targets | `dashpay/dash-evo-tool` and `dashpay/platform` PRs by `lklimek` / `Claudius-Maginificent` (P-CAP, P-CAP2, P-REV, P-REF, P-LARGE, P-XREPO); on real PRs only pending comments, deleted afterwards, never submitted | §6.1, S3, S7, M2, M3b, WP4-REAL, WP4-GUI |
| Comment outside the diff | File-level comment in the pending review, body links to the line (S3 confirms) | S3, WP3-REVIEW, WP3-INSPECTOR |
| Sign-in | Allowed GitHub logins are a tenant setting; v1 has no UI or config for it; seeded tenant `default` allows exactly `lklimek` | §5.3, WP1-A, WP2-AUTH |
| Progress before n is known | Indeterminate bar, "Planning flows…" | WP3-SCREENS |
| Failure before any flow | Failed state with a plain description and Retry | WP3-PIPE, WP3-SCREENS |
| Cancel | Partial results discarded; Home shows "Analysis cancelled — analyze again" | WP3-PIPE, WP3-SCREENS |
| First Codex login | Manual one-time command in the runner container (runbook); no UI | §6.1 O3, WP4-DOCS |
| "LLM working" in Settings | Outcome of the most recent LLM call (time; plain message on error); no on-demand check | §5.3, WP2-JOBS, WP3-STATE |
| LLM status of threads | From `analyse_flow` for threads located in its steps; other threads have no status | WP3-PIPE, WP3-PROMPTS |
| Ask during re-analysis | Claimed before the next batch job; a running job is not pre-empted | WP3-ASK |
| Numbers like "80% likely" | Forbidden in the prompt; no hard rejection | WP3-PROMPTS, WP3-VERIFY |
| Test code inside production files | LLM-classified; own flow at the end labelled "LLM-classified tests"; never hidden. Rust/Go plugins in v2 | WP3-PROMPTS, WP3-VERIFY, WP3-BLOCKS, §13 |
| Git and releases | Branch `feat/v1` only; no pull requests until v1 is deployed; single rewritable `0001_init.sql`; contracts reshaped by CCR without compatibility; no versioning or changelog; handover in `docs/v1-handover.md` | §1 Git flow, §4.3, §10 |
| Build models | All implementation agents Opus; independent reviews Codex Astra (xhigh for contracts, threat model, security-sensitive WPs, merge points, final review; high otherwise); fallback Sol high | §4.2, §12.2, §15 |
| Visual checks | Delegated to a UX agent against the mockups; no owner wait | §12.5 |
| GUI tests | Mandatory exploratory + scripted tests of the real deployed stack in a real browser at the end | §10 WP4-GUI, §12.6 |
| Owner availability | Away during implementation; all owner input collected once before dispatch; coordinator authority in §1 | §1, §6.1 |

## Appendix A. Review dispositions (`/data/artifacts/smart-review/2026-11-16/reviews/plan-review-fable.md`)

| Finding | Disposition |
|---|---|
| B1 | S7 (real PR, draft schemas, one resumed session) in the stage-0 gate; schema smoke per job kind in WP1-A acceptance; minimal Codex `run` in WP2-RUN and a real-Codex slice smoke at M2. |
| B2 | §11 traceability; WP3-STATE (review state, settings, diff) and WP3-E2E (full-stack extension, fake MemCan); threads/checks repos to WP3-REVIEW; `analysis` public signatures in WP1-A. |
| M1 | WP2-CORE split (JOBS, AUTH, INGEST), WP3-GH (AUTH, APPTOKEN, REVIEW), WP3-PRE (PREPASS, VERIFY), WP3-PIPE (PIPE, PROMPTS, ASK), WP3-UI-CODE (CODEVIEW, DIAGRAM), WP3-UI-COMP (BLOCKS, INSPECTOR), WP3-UI-SCREENS (SCREENS, TOUR); WP1-S split from WP1-A; slice scope stated as thin. |
| M2 | S5 in the gate; S1 split into S1a/S1b; device login collected in the owner session; host `~/.codex` forbidden. |
| M3 | Hand-written: repo trees, GitHub responses, agent goldens, slice-only API seed; API default generated by the server at M2; other states as merge-patch overlays; endpoint × state inventory; route ↔ msw handler test; round-trip comparison instead of `deny_unknown_fields` (simpler, no attribute on every type); 17 steps decided (mockup's 14 + 3 for line coverage); fixture ownership in §4.3. |
| M4 | §4.3 collision table incl. lockfiles, manifests, `lib.rs`, `AppState`/`Config`, schema file, mocks; skeleton pre-declares deps/state/modules; fake-github contract test in server tests; `embed-ui` feature chosen over a committed empty `web/dist` (no build-output churn); configurable clone base + `file://`/`git daemon` remote; `e2e/**` owned sequentially by WP2-E2E then WP3-E2E. Hidden dependencies in the Depends column. |
| M5 | Standing block covers Docker builds, `CARGO_BUILD_JOBS=2`, web worker caps, slot-wait re-run. Simpler than "only the coordinator builds images": the ops agent builds when the coordinator grants the slot. |
| M6 | §5.4 security design and WP1-T threat model before the freeze; tests in WP3-RUN/WP3-OPS/WP3-APPTOKEN; compensating controls for sandbox-off; runner secret named in T1. |
| M7 | S3/S4 record sanitised responses for the fake; real-GitHub and installation-token smokes at M3b with the owner-session token. |
| M8 | Plan + CLAUDE.md committed first; the owner pushes `main`; branch `feat/v1` only, coordinator gates, no pull request until deployment (§1). |
| M9 | Mockup tokens ported in WP2-WEB; delegated visual conformance checks at M2 and M3a (§12.5); GUI tests of the real stack (§12.6). |
| M10 | `basis` (fact or inference) + evidence rule in contracts and verify; rendered-text checks in Playwright; grep limited to `web/src`; tooltip sweep, Unexplained-first and inspector-never-empty tests. |
| m1 | Agent type names corrected. |
| m2 | Tracking file is the single record; MemCan for decisions only; merge-point reviews cover merge resolutions; job schemas dropped. |
| m3 | Decided in §5.2 (ADR-001): runtime sqlx queries, npm, `SR_SECRET_KEY`, configuration variables, `select_model` as a plain function (Sol, high). |
| m4 | Drop list next to the `prior-art.md` reference (§7). |
| m5 | "Changed line without annotation" in WP3-REVIEW goal and test. |
| m6 | `lib.rs`/`main.rs` stubs owned by WP1-S. |
| m7 | Server-generated golden replaces "fields that exist so far". |
| m8 | Spike Dockerfile, compose, JSONL and scripts archived with the evidence. |
| Missing questions 1–8 | Answered by the owner (§16). |
