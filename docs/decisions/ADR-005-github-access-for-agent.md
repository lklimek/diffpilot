# ADR-005: GitHub access for the agent — per-job installation token

## v1.3 container amendment (2026-11-24)

Workers use the image's read-only `gh` wrapper, with the PR-repository read-only installation token in a
per-job tmpfs file. Real user tokens and the App key remain server-only. No `github-mcp-server` or token in
the worker environment. Minting, response validation, one-hour expiry and revocation remain unchanged.
Related public searches use `gh`; the token's repository scope does not expand. The worker can read its job
token file, an accepted bounded risk under SG2/AR-13. Analytic MemCan permits search/write, Ask search only,
verify/judge none; the full bearer remains the accepted MC1 risk.

This amendment and [ADR-025](ADR-025-container-runtime.md) take precedence over the historical text below.


> Amended for v1.3 by [ADR-025](ADR-025-container-runtime.md): per-payer containers, app-server,
> gateway-only real credentials, file-based read-only GitHub tokens and host report 4.2.
> KL1-A is closed: per-user ChatGPT-plan linking only; no API-key mode.


- Status: accepted (coordinator, 2026-11-16, after spikes S4, S5 and the threat model WP1-T)
- Source: `docs/spikes/S4-app-tokens.md`, `S5-github-mcp.md`, `docs/contracts/threat-model.md`; brief §5 GitHub
  access for the agent; plan §5.4 T3.

## Decisions
- **Scope: the PR repository only.** The server mints the token from the installation that covers the PR
  repository with `repositories = [PR repo]`. Related repositories are never in the token; the agent reads them as
  public data (S4: installation tokens read any public repository; v1 supports public repositories only). This
  refines T3 and supersedes the spike proposal "[PR repo] ∪ (related ∩ installation)".
- **Every mint is explicit:** `repositories` and `permissions` (`contents`, `issues`, `metadata`,
  `pull_requests` = `read`) are always sent. A mint without `permissions` returns the App's write permissions, so
  a response whose permissions or repositories differ from the request is rejected (tested in WP3-APPTOKEN).
- **Minted at claim, not at submit.** A job may wait in the queue; the token is minted when a runner claims the
  job and placed in `JobSpec.access.github` (`InstallationToken { token, expires_at, repository }`). The stored
  job spec never contains it; an encrypted copy is kept per job attempt (`installation_tokens`) only to revoke it.
- **Revoked** (`DELETE /installation/token`) when the job result arrives or its lease expires. The copy is
  deleted only after the revocation succeeded (or the token expired); tokens still listed are revoked again at
  start (`docs/contracts/runner-protocol.md`).
- **Lifetime.** Fixed at 1 hour by GitHub; no re-mint in v1. Job runtime, compaction included, is capped at
  45 minutes (`JobLimits.max_runtime_secs = 2700`).
- **Opaque strings.** Tokens have no length limit or format assumptions (S4: 390-character `ghs_` tokens).
- **github-mcp-server.** v2.1.0, checksum-pinned in the runner image, started by Codex from the per-job config as
  stdio with `--read-only` and a `--tools` allow-list (repeated in `enabled_tools`); `get_me` excluded. Binary
  path, tool list and proxy environment are runner configuration, not `JobSpec` data. The token is written only
  into `env` of `[mcp_servers.github]` in the per-job `config.toml` (mode 0600), never into Codex's environment.
  For the owner: [accepted risk] the agent runs as the same uid as Codex and can read the per-job config, hence
  the token; accepted under T9 (WP1-T words it).
- **PR repository without the App.** No token is minted from another installation: `JobSpec.access.github` is
  none and the job has no `github-mcp-server`. The analysis still runs; the related-work lookup is skipped and the
  PR context shows it as not checked (`PrContextFacts.related_work_checked = false`); commenting is disabled
  (`commenting_available = false`).
- **MemCan for the agent** is restricted to search and `add_memory` by the runner's MCP configuration
  (`enabled_tools`), not by `JobSpec`; URL and token arrive in `JobSpec.access.memcan` at claim.

## Addendum 2026-11-20: read-only token for dependency repositories
- **Decision (owner-accepted).** The agent's read-only installation token is extended to the dependency
  repositories of controlled organisations (the controlled orgs list in Settings, ADR-021), so the PR-context agent
  can look up upstream issues and pull requests and ask whether a change belongs in the crate that defines a type.
- **v1.2 mechanism: prompt and documentation, no mint change.** v1 analyses public repositories only, and an
  installation token already reads every public repository (S4 §1, S5 §2: REST, GraphQL, search, CI). The
  `pr_context` prompt names the controlled dependency repositories and the crates and types of the conversion
  facts as lookup targets; findings use the existing `related_work` and `fit_questions` (`layer_or_repository`,
  with evidence), phrased as questions, never verdicts. The mint request stays `repositories = [PR repo]`.
- **Budget.** At most 5 dependency repositories and 3 searches each per job (prompt rule): the installation's
  shared 5,450/h core and 10/min semantic-search budgets are unchanged.
- **Later, with private repositories** (plan §18 L5): mint `repositories = [PR repo] ∪ (controlled dependency
  repositories ∩ the same installation)`, at most 10, permissions unchanged (`read`); the minted set must equal
  the requested set. Repositories of another installation stay public reads or "not checked" (one token per
  `github-mcp-server`, never a token from another installation). Precondition: the T9 egress allowlist, because a
  stolen token would then disclose private dependency code (AR-4 grows from public reads to private disclosure).
- **Threat model.** B1 gains upstream issue and pull-request text as untrusted input (fenced like other related
  work); AR-4 is unchanged in v1.2.
