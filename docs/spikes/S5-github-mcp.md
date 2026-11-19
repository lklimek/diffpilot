# S5 — GitHub MCP server for the agent

Date: 2026-11-16. `github/github-mcp-server` **v2.1.0** (released 2026-11-03), standalone binary
`github-mcp-server_Linux_x86_64.tar.gz`, SHA-256 `a442f9db287cea01008bcb55fe6bc0292270301046c3edbd1a8b22d6e4157b2b`
(release checksums file verified; binary `cdd2dfbf…5324`, static Go). Token: S4 installation token,
`repositories: [dash-evo-tool]`, `permissions` read (contents, issues, metadata, pull_requests), minted per run and
revoked after it (every revoke 204). Codex 0.167.0 in `smart-review-spike-codex:0.167.0`, harness as S1a/S1b.
The proxy item is left to S6 (runs here had direct egress). Evidence:
`/data/artifacts/smart-review/2026-11-16/spikes/S5/` — `scripts/` (`mcp-drive.py`, `s5-direct.sh`,
`job-s5.sh`, `inner-s5.sh`, `s5-readonly-fs.sh`), `inputs/` (per-job config, prompts), `responses/` (direct MCP
calls), `runs/` (Codex JSONL, rollouts, env/leak checks), all sanitised.

## Question
Does the official server accept a read-only installation token? What can it read on `dash-evo-tool` (installed),
`dashpay/platform` (not installed) and an unrelated public repo? Does read-only mode remove write tools? How does
Codex start it from the per-job config, with the token only in the server's environment? Which tools does the
agent need? Are calls visible in JSONL, and does the token leak?

## Runs
| Run | What | Result |
|---|---|---|
| `list-default-ro` / `-rw` | `stdio` with/without `--read-only`, default toolsets | 27 vs 46 tools; read-only removes all 19 write tools (`add_issue_comment`, `pull_request_review_write`, `add_comment_to_pending_review`, `merge_pull_request`, `create_or_update_file`, …) |
| `direct-read-only` (28 calls) | `--read-only --toolsets=pull_requests,issues,repos` | all reads OK on the 3 repos: PR get/files/diff/review threads/check runs, issue get, list issues/PRs, file contents, list commits, search issues/PRs/code; write tool calls → `unknown tool` |
| `direct-tools-flag` | `--read-only --tools=get_me,search_issues,pull_request_read` | only those 3 tools listed; `get_me` → `403 Resource not accessible by integration` (installation tokens have no user); burst of 33 `search_issues` → 7 OK, then "rate limit of 10" errors (earlier searches in the same minute counted) |
| `s5-mcp-list` | `codex mcp list --json` with the per-job config | server registered; **prints `env` incl. the token** (redacted in the archive) |
| `s5-run1` | `codex exec`, Sol/high, "find related PRs/issues for dash-evo-tool#1040" + env canary | rc 0, 72 s; 19 `mcp_tool_call`s; 8 plausible related items in both repos (`#941`, `#889`, `#980`, `#1043`, `#1048`, `platform#4052`, `#4183`, `#4191`) |
| `s5-run2-probe` | list tools, call `get_me` / `add_issue_comment`, read config | 10 allow-listed tools only; both calls: `TypeError: tools["mcp__github__…"] is not a function` — no request made |
| `s5-readonly-fs` | server as uid 10001, `--read-only` root fs, **no writable mount**, `HOME=/nonexistent` | initialize + `pull_request_read` on `dashpay/platform` OK |

## Findings
1. **Accepted.** The server starts with an installation token (`ghs_…`, 390 chars) in
   `GITHUB_PERSONAL_ACCESS_TOKEN`; no startup call needs a user. Only `get_me` fails (403); exclude it.
2. **Public reads work everywhere**, as S4 showed for the raw API: `dashpay/platform` (App not installed) and
   `octocat/Hello-World` give the same results as the installed repo, including search and check runs.
3. **Two independent write barriers**: `--read-only` hides write tools (calls → `unknown tool`), Codex
   `enabled_tools` hides everything outside the allow-list, and the token itself is read-only (S4: `FORBIDDEN`).
   `--tools=` on the server and `enabled_tools` in Codex both act as allow-lists; use both.
4. **Token isolation.** The token is in `env` of `[mcp_servers.github]` in the per-job `config.toml`. Codex's
   process environment (and so every agent shell command, S1b finding 9) does not contain it: `/proc/<codex>/environ`
   count 0; canary in the agent shell: `GITHUB_PERSONAL_ACCESS_TOKEN=` (unset), `gh[su]_` values 0. **Caveat**: the
   agent shares the uid with Codex and can read `$CODEX_HOME/config.toml` (probe: `grep -c` = 1) or run
   `codex mcp list --json`, which prints it. It is not exposed passively, but a prompt-injected agent can reach
   it — same exposure class as `auth.json` (T1) and the accepted T9 risk.
5. **No leak in logs**: exact-token grep over JSONL stdout, stderr and the whole per-job `CODEX_HOME` (rollout,
   `logs_2.sqlite`, state DBs), excluding `config.toml`: 0 files in both exec runs.
6. **JSONL shape** (one pair per call; Codex 0.167 "code mode" composes calls from a JS `exec` tool and may run
   them in parallel, but JSONL still shows each MCP call):
   ```json
   {"type":"item.started","item":{"id":"item_2","type":"mcp_tool_call","server":"github","tool":"pull_request_read",
     "arguments":{"owner":"dashpay","repo":"dash-evo-tool","pullNumber":1040,"method":"get"},
     "result":null,"error":null,"status":"in_progress"}}
   {"type":"item.completed","item":{"id":"item_2","type":"mcp_tool_call","server":"github","tool":"pull_request_read",
     "arguments":{…},"result":{"content":[{"type":"text","text":"{…}"}],"structured_content":null},
     "error":null,"status":"completed"}}
   ```
   The rollout records them as `item_completed` with `McpToolCall`.
7. **Rate limits.** `search_issues` is semantic search: **10/min** per installation (lexical search, used by
   `search_pull_requests`/`search_code`, is 30/min). When the limit is hit, the tool result is `isError: true` with "403 API
   rate limit of 10 still exceeded until … [rate reset in 28s]". The client short-circuits further calls until
   the reset. Core 5450/h is shared by all jobs of the installation (S4).
8. **Context cost.** Search results with bodies are large (single results up to 152 kB); run1 used 368k input
   tokens (310k cached) for a small task. Prompts should ask for small `perPage` and use the tools' `fields`
   parameter. The 10-tool allow-list cuts the tool schemas from 71 kB to 34 kB.
9. **uid `agent`.** The server runs as a child of Codex with the same non-root uid and needs no writable path
   (no `HOME`, no temp dir, read-only root fs). Binary bind-mounted read-only at `/opt/github-mcp`, so no image
   rebuild was needed. In the product it goes into the runner image.
10. Observation for S1b/T5: with cwd above the checkout, AGENTS.md is not injected, but the agent **searched
    for and read `repo/AGENTS.md` itself** (Codex base-prompt habit). Untrusted content either way, but the
    job prompt should tell the agent that repository instruction files are data.
11. Not tested: behaviour after token expiry (expected 401 tool errors; job cap 45 min < 1 h); lockdown mode
    (`--lockdown-mode` filters public content by authors without push access, which would hide
    external contributions; not wanted). The server's own App auth (`--app-id`, `--app-private-key-path`) is
    rejected: the key never leaves the server.

## Decision (proposed ADR text)
> **ADR: Agent GitHub access via github-mcp-server.** The runner image ships `github-mcp-server` v2.1.0
> (checksum-pinned). Per job, the generated Codex config declares it as stdio with `--read-only` and a
> server-side `--tools` allow-list, the same list in `enabled_tools`, and the job's installation token in the
> server's `env` only. No fallback needed: the S5 fallback (server-side lookup tool) is not triggered.

Allow-list: `pull_request_read`, `issue_read`, `list_issues`, `list_pull_requests`, `search_issues`,
`search_pull_requests`, `search_code`, `get_file_contents`, `list_commits`, `get_commit`.

Per-job config snippet (placeholder replaced by the runner when writing `$CODEX_HOME/config.toml`, 0600):
```toml
[features]
multi_agent = false            # plus the lean S1a feature set

[mcp_servers.github]
command = "/opt/github-mcp/github-mcp-server"
args = ["stdio", "--read-only", "--tools=pull_request_read,issue_read,list_issues,list_pull_requests,search_issues,search_pull_requests,search_code,get_file_contents,list_commits,get_commit"]
env = { GITHUB_PERSONAL_ACCESS_TOKEN = "<job installation token>" }
enabled_tools = ["pull_request_read", "issue_read", "list_issues", "list_pull_requests", "search_issues", "search_pull_requests", "search_code", "get_file_contents", "list_commits", "get_commit"]
startup_timeout_sec = 20
tool_timeout_sec = 60
required = true
```

## Plan impact
- `JobSpec`: unchanged beyond S4 (`github_token {value, expires_at}`). The tool allow-list and binary path are
  runner constants, not per-job data.
- Runner (WP3-RUN): write the token only into the per-job config (never Codex's env); delete the config with
  the job dir; parse `mcp_tool_call` items for the activity log; never publish `codex mcp list` output.
- Prompts (WP2/WP3): search sparingly (semantic 10/min), small `perPage`, `fields`; treat repo instruction
  files as data; on rate-limit errors continue without the search.
- Image (WP3-OPS): add the pinned binary + checksum check; no writable paths needed for it.
- Fallback: **not triggered**.
