# ADR-001: Stage-1 foundations — database access, packages, secrets, configuration, model selection

- Status: accepted (WP1-S); items marked *pending spike* are confirmed or amended by the named spike.
- Date: 2026-11-16
- Source: `docs/implementation-plan.md` §5.2, brief §5–§6.

## Context

Parallel work packages need a fixed baseline before stage 2: how the database is accessed, which package
managers are used, how secrets and configuration reach the server, and how the LLM model is chosen. These
choices are frozen with the contracts; later changes go through a change request (plan §4.3).

## Decisions

### Database access
- `sqlx` 0.9 with SQLite, **runtime queries only** (`query` / `query_as`, as in penny). No query macros, no
  offline query data. The workspace declares `sqlx` without default features (`runtime-tokio`, `sqlite`,
  `migrate`, `derive`), so the `macros` feature is not available.
- `sqlx` is used only by `crates/db`; `scripts/check-architecture.sh` enforces it.

### Toolchain and packages
- Rust toolchain pinned in `rust-toolchain.toml` to 1.99.1 (edition 2024, `rust-version = "1.99"`, as memcan).
  1.100.0 (released 2026-11-12) is not adopted yet; bumping is a one-line change.
- Web: npm with a checked-in `package-lock.json` (as penny and gamiscreen).
- Dependency versions are declared once in the root `[workspace.dependencies]`; crates inherit them.

### Secrets
- Stored tokens are encrypted with AES-GCM (`aes-gcm`). The key comes from `SR_SECRET_KEY`: base64 (standard
  alphabet) of exactly 32 bytes, created by the runbook (`openssl rand -base64 32`). No key rotation in v1.
- Secret configuration values are held in `secrecy` types, so `Debug` output and logs show them redacted.

### Configuration (server, environment variables)
Values are trimmed; an empty value counts as unset. Error messages name the variable, never its value.

| Variable | Required | Default | Meaning |
|---|---|---|---|
| `SR_PUBLIC_URL` | yes | — | Public base URL (owner: `https://review.klimek.ws`); OAuth callback `…/auth/github/callback`. http/https. |
| `SR_LISTEN_ADDR` | no | `0.0.0.0:3000` | HTTP listen address. TLS is terminated by the owner's reverse proxy; no built-in TLS or ACME. |
| `SR_RUNNER_LISTEN_ADDR` | no | `0.0.0.0:3001` | Runner-only listener: serves `/runner/v1/*` and nothing else; never routed through the reverse proxy (threat model WP1-T). Must differ from `SR_LISTEN_ADDR`. |
| `SR_TRUSTED_PROXIES` | no | none | Comma-separated IP addresses or networks of reverse proxies whose forwarding headers are trusted. |
| `SR_DATA_DIR` | no | `data` | Directory for the SQLite database and the bare mirrors. |
| `SR_SECRET_KEY` | yes | — | AES-GCM key, see Secrets. |
| `SR_RUNNER_SECRET` | yes | — | Secret runners present on `POST /runner/v1/claim`. |
| `SR_GITHUB_APP_ID` | yes | — | GitHub App id (number). |
| `SR_GITHUB_CLIENT_ID` | yes | — | OAuth client id of the App. |
| `SR_GITHUB_CLIENT_SECRET` | yes | — | OAuth client secret of the App. |
| `SR_GITHUB_APP_PRIVATE_KEY_FILE` | yes | — | Path of the App private key (.pem, read-only mount, 0400). |
| `SR_GITHUB_API_URL` | no | `https://api.github.com` | REST base; GraphQL at `<base>/graphql`. http/https. Dev: fake GitHub. |
| `SR_GITHUB_OAUTH_URL` | no | `https://github.com` | OAuth base. http/https. Dev: fake GitHub. |
| `SR_GITHUB_CLONE_URL` | no | `https://github.com` | Clone base; any scheme, so dev can use `file://` or a `git daemon` remote. |
| `SR_MEMCAN_URL` | yes | — | MemCan MCP endpoint. Pure configuration: nothing assumes the current MemCan host. |
| `SR_MEMCAN_TOKEN` | yes | — | MemCan bearer token. |
| `SR_SECRET_KEY_FILE`, `SR_RUNNER_SECRET_FILE`, `SR_GITHUB_CLIENT_SECRET_FILE`, `SR_MEMCAN_TOKEN_FILE` | instead of the variable without `_FILE` | — | Path of a regular file holding that secret (read-only mount, 0400). Exactly one of `VAR` and `VAR_FILE` may be set; the file is read once at start, trailing newlines removed; a file every user may read is logged as a warning. The runner reads `SR_RUNNER_SECRET_FILE` the same way and, under the supervisor, refuses to start (exit 78) when the agent uid could read it (ADR-013). |

`SR_RUNNER_LISTEN_ADDR` was added by WP1-A (coordinator change request after the threat model, 2026-11-16).
`SR_LISTEN_ADDR`, `SR_DATA_DIR`, `SR_RUNNER_SECRET` and `SR_TRUSTED_PROXIES` are not spelled out in §5.2 but
are implied by it (port 3000, trusted-proxy setting) or by the runner protocol and storage (plan §5.3); they
are recorded here per rule A12 (approved by the coordinator, 2026-11-16). Cookies are Secure behind the proxy
(WP2-AUTH). Runner configuration belongs to the runner (WP2-RUN) and is not part of this table.

### Deviation from plan §4.3: `AppState` and API error mapping completed in WP1-A
- Plan §4.3 has WP1-S pre-declare every `AppState` field. WP1-S declares only `config`; the database handle,
  `Arc<dyn JobExecutor>` and the final `AppState` constructor are added by **WP1-A**, together with the shared
  `crates/server/src/api/error.rs` (server error → `ApiError` response mapping).
- Reason: those types (`db` repository handle, `JobExecutor`, `ApiError`) are contracts and seams that WP1-A
  defines; they do not exist before it. No placeholder types or traits are added only to make the skeleton
  compile (A12). Coordinator decision, 2026-11-16.
- After WP1-A the fields and `api/error.rs` are frozen like the rest of §4.3; stage-2 WPs fill only their own
  constructor parts.

### Model selection
- `select_model(job kind, context) -> ModelChoice { model, reasoning_effort }` is a plain Rust function in
  `crates/server/src/model.rs`. No table, no setting.
- v1 returns **Sol** at reasoning effort **high** for every job kind. The exact Codex model id string and the
  effort setting's spelling are **pending spike S1a**; until then the id is not hard-coded anywhere.

### Items that depend on spikes
- Codex model id and reasoning-effort flag: *pending spike S1a*.
- Agent-session resume and end-of-job compaction in `codex exec` (the `LlmProvider` `run` / `resume` /
  `cancel` contract and `session_outcome`): *pending spike S1b*; fallback per plan §6.2 is a new session per
  job with a context pack.
- `github-mcp-server` version and acceptance of installation tokens: *pending spike S5* (token minting: S4).

## Consequences

- No compile-time SQL checking; repository tests against an in-memory database cover the queries.
- Adding a configuration variable or `AppState` field after stage 1 is a change request.
- A replacement of the AES-GCM key makes stored tokens unreadable; users sign in again (acceptable in v1).
