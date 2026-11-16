# CLAUDE.md — smart-review

Self-hosted web tool that helps a human review a GitHub PR: final code with changed regions grouped into flows,
walked step by step, with LLM analysis (Codex CLI) clearly separated from deterministic facts.

## Read first
- `docs/design-brief.md` — the approved design; authoritative. Mockups: `docs/mockups/` (brief wins).
- `docs/implementation-plan.md` — stages, work packages, contracts, architecture guard. Coordinators start at its §1.
- `docs/contracts/`, `docs/decisions/` (ADRs), `docs/spikes/` — once they exist.

## Architecture rules (never break; details in plan §3–§5)
- Crates: `contracts` (wire types, no IO) <- `analysis`, `db`, `runner`; `server` -> `contracts`, `analysis`, `db`.
  Nothing depends on `server` or `runner`. `runner` has no DB access and no GitHub token.
- All database code (sqlx, migrations, row types) lives in `crates/db` behind its repository API; nothing else uses sqlx.
- Pipeline and Ask use only the `JobExecutor` trait; `JobSpec` is self-contained; runner core is
  `execute(JobSpec, EventSink) -> JobResult`.
- `LlmProvider` is `run` + `resume` + `cancel`: one agent session per analysis, resumed by every batch and Ask
  job, compacted at job end; a lost session falls back to a context pack. Session files stay per analysis.
  Model and effort come from the server's `select_model` (v1 placeholder: Sol, high).
- Only `runner::provider` knows Codex; only `analysis::tech` (TechPlugin) knows path/line rules and call hierarchy
  (`generic` returns `None` in v1).
- Every UI block is a self-contained component in `web/src/components/`: props in, callbacks out, no fetching,
  no router; screens only arrange.
- Contract types come from `crates/contracts`; generated JSON Schemas and TS types are checked in and must be current.
  Contract, migration, route or trait changes need coordinator approval (change request).
- Trust and wording rules of brief §1 are tested on rendered text and on agent output: badges only for
  deterministic facts, no numeric confidence, no "no risk"/"verified"/"noise", facts carry evidence, questions
  not verdicts, assessment collapsed.
- Runner security design (plan §5.4): agent runs as a separate uid without access to runner secrets, cache or
  master `CODEX_HOME`; MCP token separate from the job token; never mount the host `~/.codex`.
- Keep it simple: no abstraction beyond the seams the brief names.

## Build and test
To be filled in stage 1 (workspace, web, generated-artefact update, architecture and wording checks).

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
- Git: one feature branch `feat/v1` and one draft PR for v1; conventional commits; agents commit and never
  push; the coordinator integrates and pushes `feat/v1` (`ghsudo git push`); never push to `main` (the owner
  merges the PR); no force-push, no amend. Code-mutating agents work in pre-created worktrees
  `/data/git-worktrees/home-ubuntu-git-smart-review-<stream>`. Changes of 50+ code
  lines get an independent review (`claudius:review-changes`).
- Storage: heavy scratch in `/data/tmp`; durable artefacts in `/data/artifacts/smart-review/YYYY-MM-DD/`;
  no large files in `/tmp`.
- Language: code, comments, commits, PRs and docs in English; talk to the owner in Polish.
- Real dashpay PRs (plan §6.1 test targets): only pending review comments, deleted afterwards; never submit.
- Test isolation: temp dirs and in-memory SQLite, fake GitHub and MemCan (`tools/`), fake LLM provider (feature
  `fake-provider`, never in release builds); never real tokens in tests or fixtures.

## Versioning
None yet; no releases or tags until the owner decides.
