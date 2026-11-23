# CLAUDE.md — diffpilot

Self-hosted web tool that helps a human review a GitHub PR: final code with changed regions grouped into flows,
walked step by step, with LLM analysis (Codex CLI) clearly separated from deterministic facts.

## Read first
- `docs/design-brief.md` — the approved design; authoritative. Mockups: `docs/mockups/` (brief wins).
- `docs/implementation-plan.md` — stages, work packages, contracts, architecture guard. Coordinators start at its §1.
- `docs/contracts/`, `docs/decisions/` (ADRs), `docs/spikes/` — once they exist.
- `docs/v1.1-dashboard-plan.md` — stage v1.1 (GitHub work dashboard, ADR-016..020); mirrors the implementation plan's rules.
- `docs/review-workflow-plan.md` — stage v1.3 (processing PR comments: PR page, Comments, Watch, decision feed; ADR-023).

## v1.3 common contract amendment
- [ADR-025](docs/decisions/ADR-025-container-runtime.md) and [container contracts](docs/contracts/containers.md)
  govern W1 CCR1–CCR13; downstream W3/W4/W5/W8 implement runtime behavior and W9 removes the old runtime.
- Containers replace the uid/supervisor boundary (SG1–SG16); no real provider credential enters a worker.
  Agent GitHub access uses `gh` and a per-job read-only tmpfs token file; hosted web search stays disabled.
- Workers egress via Squid, with LAN denied except MemCan. Gateway reaches fixed upstreams directly.
- Analytic sessions belong to (PR,payer); Ask forks only its payer's analytic volume, otherwise uses a context pack.
- Only `runner::provider` and `server::gateway` know Codex. Runner gains launcher mode and gateway admin client,
  still no DB or GitHub API client. Gateway may use secrets/config/contracts, never db.
- Full fixed thread config on start/fork/resume; existing threads attach/resume before operations; CS1 cwd `/work`.
- Server and worker checker format v3 are rebuilt together. JobSpec stays self-contained; model selection,
  fake provider and configured MemCan stay. Auth tests are fake-only; gateway lifecycle follows ADR-025.
- KL1-A selects gateway-run ChatGPT-plan device linking only; CCR10 rejects API keys, secrets and base URLs.
- CCR11–CCR13 freeze dual-method assessment passes with primary-only effects and atomic 80/day UTC quota.
- Owner decision 2026-11-23: 0030 upgrades the existing DB in place, cleaning analysis/runtime data while
  preserving settings, identity and GitHub auth (table inventory: container contracts). Codex relinking is required.
  Only unmerged 0030 may be edited; applied migrations remain append-only.

## Architecture rules (never break; details in plan §3–§5)
- Crates: `contracts` (wire types, no IO) <- `analysis`, `db`, `runner`; `server` -> `contracts`, `analysis`, `db`.
  Nothing depends on `server` or `runner`. `server` may dev-depend on the test tool `fake-github` (tests only). `runner` has no DB access and no GitHub client code.
- All database code (sqlx, migrations, row types) lives in `crates/db` behind its repository API; nothing else uses sqlx.
- Pipeline and Ask use only the `JobExecutor` trait; `JobSpec` is self-contained; runner core is
  `execute(JobSpec, EventSink) -> JobResult`.
- `LlmProvider` is `run` + `resume` + `cancel`: one analytic session per pull request (analysis and repairs
  only), plus one private working session per (PR, user) for Ask. Re-fork when the analytic generation advances.
  Codex compacts by itself at 400k tokens; a new Ask fork's first turn uses 80k (one pre-turn compaction, ADR-004 addendum). Each successful useful turn is archived; a lost working session forks the analytic archive, then falls back to a private context pack.
  Model and effort come from `select_model`, a plain Rust function in the server using the stored
  installation-wide Settings choice (default Sol, high; Ask: stored Ask effort, default medium). Each new job reads the current choice.
- Flow jobs write tagged `out/flow.md` and run `sr-check`; server and checker share `analysis::tagged` (ADR-014).
  Format version 3 (incidental changes): server and runner images are rebuilt together. Plan, PR context and Ask keep JSON.
- Only `runner::provider` knows Codex; only `analysis::tech` (TechPlugin) knows path/line rules and call hierarchy
  (`generic` returns `None` in v1).
- Every UI block is a self-contained component in `web/src/components/`: props in, callbacks out, no fetching,
  no router; screens only arrange.
- Contract types come from `crates/contracts`; generated JSON Schemas and TS types are checked in and must be current.
  Contract, migration, route or trait changes need coordinator approval (change request).
- Trust and wording rules of brief §1 are tested on rendered text and on agent output: badges only for
  deterministic facts, no numeric confidence, no "no risk"/"verified"/"noise", facts carry evidence, flags and questions
  not verdicts, assessment collapsed.
- GitHub credentials: user tokens and the App private key never leave the server. The agent holds only a
  short-lived read-only installation token (minted per job by the server, PR repository only (ADR-005), 1 h) for the
  official `github-mcp-server` in read-only mode.
- Runner security design (plan §5.4): agent runs as a separate uid without access to runner secrets, job
  token, cache or master `CODEX_HOME`; never mount the host `~/.codex`. Egress only through the Squid explicit
  proxy (v1: allow-all, host-name log, no TLS interception); Codex's hosted web search is disabled for every job;
  MemCan URL and token are pure configuration.
- Keep it simple: no abstraction beyond the seams the brief names.

## Build and test
`W` = claudius `scripts/cargo-cached.sh` with `CLAUDIUS_MAX_PARALLEL_BUILDS=1`; narrowest `-p`. CI
(`.github/workflows/ci.yml`) runs the workspace with `--features runner/fake-provider`.
- Tests: `$W nextest run -p <crate>`; runner with the scripted provider `-p runner --features fake-provider`;
  fake GitHub `-p fake-github`; embedded UI `-p server --features embed-ui` (after `npm run build` in `web/`).
  Doctests: `$W test -p <crate> --doc`.
- Clippy: `$W clippy -p <crate> --all-targets -- -D warnings` (runner: add `--features fake-provider`).
  Format: `cargo fmt --all`.
- Checked-in schemas and TS types: `UPDATE_GENERATED=1 $W nextest run -p contracts -E 'test(generated_artifacts_are_current)'`.
- Tour golden: `UPDATE_GOLDEN=1 $W nextest run -p server --test slice -E 'test(slice_pipeline)'`.
- Prompt goldens: `UPDATE_GOLDEN=1 $W nextest run -p server -E 'test(golden_prompts_of_fixture_pr_482)'`.
- Web, in `web/`: `npm ci`, `npm run lint`, `npm test`, `npm run build`; Playwright (msw): once
  `npx playwright install --with-deps chromium`, then `npm run test:e2e`.
- Checks: `scripts/check-architecture.sh`, `scripts/check-migrations.sh`, `scripts/check-wording.sh` (after
  `npm ci` in `web/`; all take `--self-test`), `shellcheck` on `scripts/*.sh tools/**/*.sh deploy/**/*.sh` (globstar), `python3 -m pytest -q scripts/tests`.
- Containers (one heavy job at a time, Docker): `R1_SUPERVISOR=rust R1_SKIP_CODEX=1 deploy/runner/r1-test/run.sh`;
  `deploy/egress-proxy/test.sh`; `deploy/test-stack.sh`; `deploy/check-production-config.sh --self-test`.
- db: transactions only through `db::repo::{write_tx, read_tx}` (write = `BEGIN IMMEDIATE`; never a deferred read-then-write).
- Schema smoke (manual, real Codex, Docker image and login volume of spike S1a): `scripts/schema-smoke.sh [kind ...]`.

## Conventions
- Rust: `thiserror` typed enums for all errors (no anyhow/eyre/`Box<dyn Error>`/String errors); no `unwrap`/`expect`
  outside tests; `tracing`; clippy `-D warnings`; `#[expect(.., reason)]`, never `#[allow]`; TDD; one-line `///`
  on public items; verify dependency versions on the registry when adding; prefer crates used in penny,
  gamiscreen, memcan.
- Heavy jobs on this host (~15 GB RAM): at most ONE at a time across all agents — cargo builds/tests and Docker
  image builds alike. `[build] jobs = 2` and `NEXTEST_TEST_THREADS=4` are preset, never raise them; run
  test/clippy/nextest via the claudius `cargo-cached.sh` wrapper at the narrowest `-p`
  (`CLAUDIUS_MAX_PARALLEL_BUILDS=1`); never set `CARGO_TARGET_DIR`; long runs in the foreground with a timeout;
  Dockerfiles set `CARGO_BUILD_JOBS=2`; vitest and Playwright at most 2 workers, one Playwright run at a time.
- Git: one integration branch per version (`feat/vX.Y`, cut from `main`) and one pull request per version, no
  small per-feature pull requests; work branches merge into the version branch; conventional commits; agents
  commit and never push; the coordinator integrates and pushes the version branch; never push to `main` (the
  owner merges it); no force-push, no amend. Migrations are append-only: a schema change is a new
  numbered file in `crates/db/migrations` that upgrades in place; never edit an applied migration (its checksum is
  verified at start; `scripts/check-migrations.sh`, `--add-new` records a new file). No versioning or changelog.
  Code-mutating agents work in pre-created worktrees
  `/data/git-worktrees/home-ubuntu-git-diffpilot-<stream>`. Changes of 50+ code
  lines get an independent review (`claudius:review-changes`).
- Storage: heavy scratch in `/data/tmp`; durable artefacts in `/data/artifacts/diffpilot/YYYY-MM-DD/`;
  no large files in `/tmp`.
- Language: code, comments, commits and docs in English; talk to the owner in Polish.
- Real dashpay PRs (plan §6.1 test targets): only pending review comments, deleted afterwards; never submit.
- Test isolation: temp dirs and in-memory SQLite, fake GitHub and MemCan (`tools/`), fake LLM provider (feature
  `fake-provider`, never in release builds); never real tokens in tests or fixtures.

## Versioning
None yet; no releases or tags until the owner decides.
