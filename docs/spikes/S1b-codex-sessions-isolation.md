# S1b — Codex sessions and isolation

Date: 2026-11-16. Same image, harness and credential handling as S1a (`docs/spikes/S1a-codex-basics.md`).
Evidence (sanitised): `/data/artifacts/smart-review/2026-11-16/spikes/S1b/`.

## Question
Can a session be resumed from a copied archive in a fresh per-job `CODEX_HOME` and work dir? How do we compact
non-interactively? Do sub-agents work in `exec`? Does SIGTERM cancel cleanly? Does the Codex sandbox work in the
unprivileged container? Can repository config be ignored? What is the MCP client config? What does the
usage-limit error look like?

## Runs
| Run | What |
|---|---|
| `s1b-a` | new session, cwd `/work`; the agent reads `calc.py` and is given codeword PELICAN-42 |
| `s1b-b` | fresh `CODEX_HOME` holding **only** `auth.json`, `config.toml` and the one rollout file; cwd `/work2` (other mount); `exec resume <id>`: recalls PELICAN-42 |
| `s1b-c` | `exec resume <id> -c model_auto_compact_token_limit=5000 "Reply OK"`: pre-turn compaction |
| `s1b-d` | resume after compaction: still recalls PELICAN-42 |
| `s1b-after-cancel` | resume of the S1a SIGTERM-cancelled session: works |
| `s1b-subagent` | `spawn_agent` + `wait` inside `exec` |
| `sandbox-matrix.txt` | `codex sandbox` with system and bundled bwrap and legacy landlock, under four Docker profiles (no quota used) |
| `pi-v1/v2/v3` | `codex debug prompt-input` against a canary repo (AGENTS.md, `.codex/config.toml`, `.agents/skills`), no quota used |
| `s1b-mcp-list`, `s1b-mcp` | stdio MCP server with `env`, `env_vars`, `enabled_tools`; streamable-HTTP entry with `bearer_token_env_var` |
| `s1b-shellenv` | which environment variables agent shell commands inherit |

## Findings
1. **Resume works from a minimal archive.**
   - The minimal file set is `sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl`. No sqlite state (`state_5.sqlite`,
     `thread_history_1.sqlite`, …) is needed, and the dated path layout is enough.
   - A different cwd is fine: the new turn records the new cwd.
   - Resume appends to the same rollout file, so the archive to copy back is that file (plus any sub-agent rollouts, finding 4).
   - Command: `codex exec resume <UUID> [--json] [--output-schema F] [-m M] [-c ...] "<prompt>"`.
   - `exec resume` has no `-s`: the sandbox comes from config.
2. **Auth with a per-job `CODEX_HOME`.** The credential gets there as a copy of the master `auth.json` (S1a ADR).
   A refresh inside a copy rotates the single-use refresh token, so the runner must write it back. Serialising
   jobs per credential is mandatory; the master stayed valid throughout (verified after each run).
3. **Non-interactive compaction.**
   - There is no `/compact` in `exec`.
   - A trailing resume with a tiny `model_auto_compact_token_limit` triggers *pre-turn* compaction: the rollout
     gets a `compacted` record and a `ContextCompaction` item. The JSONL stream shows no compaction item. The
     trivial turn after it is cheap.
   - Memory survived compaction (s1b-d).
   - The alternative is the app-server JSON-RPC `thread/compact/start`, which is heavier.
   - On a tiny conversation the context did not shrink: about 12.8k per call, dominated by the base prompt. Real
     sizes before and after compaction are for S7.
4. **Sub-agents work in `exec`** (`multi_agent` is on by default).
   - The child writes its own rollout file; `session_meta.source.subagent.thread_spawn.parent_thread_id` points
     at the parent.
   - JSONL shows only `collab_tool_call` `wait` items: `spawn_agent` is not mapped.
   - The child's usage (26.9k) is **not** in the parent's `turn.completed.usage`.
5. **Cancel.** SIGTERM gives rc 143, as in S1a. The interrupted session resumes normally.
6. **Codex sandbox in the hardened container: does not work.**

   | Docker profile | System `/usr/bin/bwrap` (Codex prefers it) | Bundled `codex-resources/bwrap` | Legacy landlock |
   |---|---|---|---|
   | default (cap-drop ALL, no-new-privileges) | fails: no permission for user namespace (seccomp) | same | Codex panics: "filesystem-restricted execution requires bubblewrap" |
   | `seccomp=unconfined` | fails: "Failed to make / slave" (AppArmor docker-default) | — | same panic |
   | `seccomp=unconfined` + `apparmor=unconfined` | **works** (workspace-write enforced, uid 10001) | fails: "loopback: Failed RTM_NEWADDR" | same panic |

   Host facts: kernel 7.0, `apparmor_restrict_unprivileged_userns=1`. Making it work means dropping the seccomp
   *and* AppArmor confinement of the container, which weakens T4 more than the Codex sandbox adds.
7. **Repository config.**
   - Project `.codex/config.toml` is not loaded: the project is untrusted and the per-job config never marks it
     trusted. Neither are its hooks or MCP servers.
   - AGENTS.md *is* injected by default. `project_doc_max_bytes = 0` removes it.
   - Repo skills under `.agents/skills` *are* discovered by default, from the project root (first `.git` upward)
     down to the cwd.
   - **Running Codex with cwd = the job dir and the checkout in a subdirectory (`/job/work/repo`) removes all
     three** (canaries absent in `pi-v3`).
   - `--ignore-user-config` skips `$CODEX_HOME/config.toml`, which we generate ourselves, so it is not needed.
     `--ignore-rules` skips execpolicy `.rules`.
8. **MCP client config** (`config/src/mcp_types.rs`; verified with a bash+jq stdio server).
   - stdio: `command`, `args`, `env = {..}`, `env_vars = ["NAME"]` (forwarded from the parent), `cwd`,
     `enabled_tools`/`disabled_tools`, `startup_timeout_sec`, `tool_timeout_sec`, `required`.
   - The server process gets a **scrubbed environment**: 7 variables plus `env` plus `env_vars`. A parent-only
     secret was *not* visible.
   - `enabled_tools = ["echo_env"]` hid the second tool from the model.
   - The call appears as `mcp_tool_call{server, tool, status, result}` in JSONL.
   - Streamable HTTP: `url`, `bearer_token_env_var = "NAME"` (or `http_headers`/`env_http_headers`), `enabled`.
     The config was accepted (`codex mcp list --json`); a live HTTP server was not exercised (left for S5 and the
     MemCan work).
9. **Agent shell commands inherit the full environment of the Codex process**: every container variable plus
   `CODEX_HOME`, `CODEX_THREAD_ID` and so on, proxy variables included. T2 therefore has to be enforced by the
   runner: start Codex with a constructed environment (`env -i` + allow-list) or set `shell_environment_policy`.
   The agent can always read `$CODEX_HOME/auth.json` (the per-job copy); T1 accepts that.
10. **Usage-limit error shape** (source; not triggered on purpose).
    - `UsageLimitReachedError{plan_type, resets_at, limit_window_minutes, rate_limits, rate_limit_reached_type}`
      exists in `protocol/src/error.rs`.
    - In exec JSONL only its Display text survives, as `error.message` plus `turn.failed.error.message`, for
      example "You’ve hit your usage limit. Visit https://chatgpt.com/settings/usage to purchase more credits or
      try again at 3:42 PM." No code and no machine-readable reset time.
    - Structured data: the rollout's `token_count.rate_limits` (`used_percent`, `window_minutes`, `resets_at`,
      `rate_limit_reached_type`), or the app-server protocol.

## Decision (proposed ADR text)
- **ADR "Agent sandbox": Codex's own sandbox is off** (`sandbox_mode = "danger-full-access"`,
  `approval_policy = "never"`). T1–T4 are the compensating controls: separate uid, scrubbed env, cap-drop ALL,
  no-new-privileges, read-only root, Squid-only egress. This is the planned fallback. "For the owner": enabling it
  would require `seccomp=unconfined` + `apparmor=unconfined`.
- **ADR "Session persistence":**
  - A session archive is the set of rollout files of the thread tree (parent plus sub-agent rollouts, linked by
    `parent_thread_id`), stored per analysis under the runner uid.
  - Restore copies them into `sessions/` of the per-job `CODEX_HOME`. Every batch or Ask job is
    `codex exec resume <thread_id>`.
  - Job end: one extra `exec resume <id> -c model_auto_compact_token_limit=<small> "<ack prompt>"` performs
    compaction. The runner then copies the rollout files back.
  - Lost or corrupt archive: new session plus context pack (planned fallback).
- **ADR "Repository config isolation":**
  - Codex cwd = the job dir and the checkout at `<job>/repo`. Codex is told the path in the prompt.
  - `project_doc_max_bytes = 0` as a second layer. The project is never trusted.
  - `features.hooks = false`. Skills come only from the runner-provided root (S2).
- **Sub-agents:** `features.multi_agent = false` for v1 jobs. Usage accounting and archives would otherwise have
  to follow child threads, and S2 shows claudius skills trigger unwanted delegation. Can be revisited later.
- **Usage limit:**
  - Detect via `turn.failed` whose message starts "You’ve hit your usage limit" or "Your workspace is out of
    credits".
  - Take `resets_at` from the latest rollout `rate_limits`, not from the text.
  - Pause the analysis until then.

## Plan impact
- `JobSpec`: thread id (nullable for a new session), the session archive reference, model and effort, a
  generated config (MCP servers with env, enabled tools), skills root, and a compaction flag.
- `LlmProvider.run`/`resume` map to `exec` and `exec resume`. `cancel` = SIGTERM. Compaction is a runner step at
  job end, not a separate provider call.
- Runner (WP3-RUN):
  - Start Codex under a constructed environment (T2).
  - Per-job `CODEX_HOME` outside `/tmp`.
  - Copy the credential in and back, serialised per credential.
  - Archive and restore the rollout files, including children.
  - Sum usage from the rollout.
- WP3-OPS: no seccomp or AppArmor relaxation needed, since the sandbox is off.
- Fallbacks triggered: **sandbox → off (T1–T4 compensate)**. Resume, compaction and repo-config ignoring all
  work without fallback.
