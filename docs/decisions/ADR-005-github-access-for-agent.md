# ADR-005: GitHub access for the agent — per-job installation token

- Status: accepted (coordinator, 2026-11-16, after spikes S4, S5)
- Source: `docs/spikes/S4-app-tokens.md`, `S5-github-mcp.md`; brief §5 GitHub access for the agent; plan §5.4 T3.

## Decisions
- **Scope.** The server mints the token from the installation that covers the PR repository, with
  `repositories = [PR repo] ∪ (related repos ∩ installation repos)`. A repository outside the installation makes
  the whole mint fail (422), so it is never passed; related repositories outside the installation are read as
  public data (S4: installation tokens read any public repository). This refines T3.
- **Permissions** are always sent explicitly: `contents`, `issues`, `metadata`, `pull_requests` = `read`. A mint
  without `permissions` returns the App's write permissions, so a response whose permissions differ from the
  request is rejected (tested in WP3-APPTOKEN).
- **Minted at claim, not at submit.** A job may wait in the queue; the token is minted when a runner claims the
  job and placed in `JobSpec.access.github` (`InstallationToken { token, expires_at, repositories }`). The stored
  job spec never contains it; an encrypted copy is kept with the job only to revoke it.
- **Revoked** (`DELETE /installation/token`) when the job result arrives or its lease expires.
- **Lifetime.** Fixed at 1 hour by GitHub; no re-mint in v1. Job runtime, compaction included, is capped at
  45 minutes (`JobLimits.max_runtime_secs = 2700`).
- **Opaque strings.** Tokens have no length limit or format assumptions (S4: 390-character `ghs_` tokens).
- **github-mcp-server.** v2.1.0, checksum-pinned in the runner image, started by Codex from the per-job config as
  stdio with `--read-only` and a `--tools` allow-list (repeated in `enabled_tools`); `get_me` excluded. Binary
  path, tool list and proxy environment are runner configuration, not `JobSpec` data. The token is written only
  into `env` of `[mcp_servers.github]` in the per-job `config.toml` (mode 0600), never into Codex's environment.
  For the owner: [accepted risk] the agent runs as the same uid as Codex and can read the per-job config, hence
  the token; accepted under T9 (WP1-T words it).
- **PR repository without the App.** Analysis is allowed with a token minted from any installation of the tenant
  (public reads work); commenting is disabled (`commenting_available = false`). No installation at all →
  `app_not_installed`.
