# S1a — Codex basics

Date: 2026-11-16. Codex CLI 0.167.0 (`codex-package-x86_64-unknown-linux-musl`, SHA-256 pinned), image
`diffpilot-spike-codex:0.167.0`, ChatGPT Pro subscription login (device auth) on volume `sr-spike-codex-home`.
Evidence (sanitised): `/data/artifacts/diffpilot/2026-11-16/spikes/S1a/`; harness: `../scripts/`.

## Question
Can an unprivileged non-root container drive `codex exec --json --output-schema` on the subscription? What is the
event stream, which JSON Schema subset is accepted, is the refreshed token written back, what is the exact model id
for "Sol" and how is reasoning effort set, which hosts are contacted?

## Runs
Every job ran as uid 10001 with `--cap-drop ALL`, `no-new-privileges` and a read-only root fs. Each job got a
fresh per-job `CODEX_HOME` (tmpfs) containing a copy of the master `auth.json` plus a generated `config.toml`. After
the job, `auth.json` was written back to the master only if it changed and the master had not changed during the
job (`scripts/inner.sh`). Egress went through a logging CONNECT proxy (`scripts/logproxy.py`).

| Run | Result |
|---|---|
| `models` (`codex debug models`) | model catalog (below) |
| `run1-success` | `--output-schema` (nullable object, enum, array of objects, depth 4), `gpt-6.1-sol`, effort high: valid JSON, rc 0 |
| `probe-noaddl`, `probe-optional`, `probe-oneof` | rejected with HTTP 400 `invalid_json_schema` before generation, rc 1 (the schema-error run) |
| `probe-anyof`, `probe-depth11`, `probe-keywords` | accepted |
| `run3-cancel` | SIGTERM after ~9 s: rc 143, stream just ends (the cancelled run) |
| `run4-refresh` | copy's access-token `exp` forced into the past: refresh at `auth.openai.com`, rotated credential written back |
| `run5-master-check` | master credential works after the rotation; `codex login status` = "Logged in using ChatGPT" |

## Evidence and findings
1. **Headless subscription auth works** in the unprivileged container. `auth.json` holds `auth_mode=chatgpt`,
   tokens (id, access, refresh) and `last_refresh`. The access token lives 240 h. Codex refreshes it within 5 min
   of `exp`, or after 8 days if `exp` cannot be parsed, or on 401 (source `login/src/auth/manager.rs`).
2. **Refresh tokens are single-use.** A refresh returns a new refresh token, which is persisted with a
   compare-and-save. A reused one fails with `refresh_token_reused` ("Your refresh token has already been used",
   `login/src/auth/util.rs`). In run4 the refresh token rotated and the write-back kept the master valid (run5). A
   copy that refreshes without write-back invalidates the master. I did not test that deliberately: reuse
   detection might revoke the whole token family.
3. **Event stream** (`--json`, `exec/src/exec_events.rs`, archived):
   - Events: `thread.started{thread_id}`, `turn.started`, `item.started|updated|completed{item}`,
     `turn.completed{usage}`, `turn.failed{error.message}`, `error{message}`.
   - Item types: `agent_message`, `reasoning`, `command_execution`, `file_change`, `mcp_tool_call`,
     `collab_tool_call`, `web_search`, `todo_list`, `error`.
   - The final structured output is the last `agent_message.text`: a JSON string, also written to `-o FILE`.
   - **`turn.completed.usage` is cumulative for the thread**, including turns of earlier resumed jobs. The per-call
     figure (`last_token_usage`) is only in the rollout's `token_count` events.
4. **Schema subset (strict mode, enforced server-side).**
   - Required: every object has `additionalProperties: false` and lists *every* property in `required`. Optional
     fields are expressed as nullable (`"type": ["object","null"]`).
   - Not permitted: `oneOf`.
   - Accepted: `anyOf`, `const`, `enum`, `format: date-time`, `pattern`, `minItems`/`maxItems`,
     `minimum`/`maximum`, recursive `$defs`/`$ref`, nesting depth 11.
   - A bad schema fails fast: an `error` event plus `turn.failed`, with message
     `Invalid schema for response_format 'codex_output_schema': ...` (`code: invalid_json_schema`), rc 1, no tokens spent.
5. **Cancel.** SIGTERM terminates `codex exec` immediately: exit 143, no `turn.failed`/`turn.completed`, and the
   rollout ends without `task_complete`. The session stays resumable (S1b). The runner must forward the signal to
   the Codex process: a shell wrapper does not (see `inner.sh` trap). Treat rc 143 plus no terminal event as
   "cancelled".
6. **Model and effort.** `codex debug models` lists, among others, `gpt-6.1-sol` ("Latest workhorse model",
   500k context, efforts low…ultra), `gpt-6-sol`, `gpt-5.6-sol`, `gpt-6-astra`. Effort is set per job with
   `-c model_reasoning_effort="high"` (or `model_reasoning_effort = "high"` in the per-job config). The rollout
   `turn_context` confirms `model: gpt-6.1-sol, effort: high`.
7. **Hosts.**
   - Model traffic: only `chatgpt.com:443` (HTTPS/SSE, no WebSocket in 0.167 default) and `auth.openai.com:443`
     (refresh only).
   - With default features a fresh `CODEX_HOME` also pulls about 60 MB of plugin and app caches from
     `*.oaiusercontent.com` and uploads analytics to `ab.chatgpt.com`.
   - The lean per-job config below removes both. It also cuts the base prompt from about 29.5k to about 12.5k
     input tokens.
8. **Per-job `CODEX_HOME` location.** It must not be under `/tmp`: Codex refuses to create its helper aliases there
   (warning). Use for example `/job/codex-home` on an `exec`-enabled tmpfs. `codex exec` reads stdin when it is not a
   TTY, so pass the prompt as an argument or via stdin and close stdin.
9. **Rate-limit telemetry is in the rollout.** `token_count` events carry `rate_limits`: limit id `codex`, weekly
   window (10080 min), `used_percent`, `resets_at`, `plan_type: pro`, and credit flags. On the day of the spike
   the weekly use was 41 %.

## Lean per-job config (used from run probe-* on)
```toml
model = "gpt-6.1-sol"
model_reasoning_effort = "high"
sandbox_mode = "danger-full-access"   # see S1b
approval_policy = "never"
project_doc_max_bytes = 0             # see S1b
[analytics]
enabled = false
[features]
plugins = false
apps = false
remote_plugin = false
plugin_discovery_prefetch = false
image_generation = false
browser_use = false
computer_use = false
hooks = false
tool_suggest = false
goals = false
daemon_auto_start = false
```

## Decision (proposed ADR text)
- **ADR "Codex model and effort":** "Sol" is `gpt-6.1-sol`, the current workhorse in the CLI catalog;
  `select_model` returns `("gpt-6.1-sol", "high")` for every kind. The id is passed as `-m`/`model` and the effort
  as `model_reasoning_effort`. If the account loses the model, `select_model` is the only place to change.
- **ADR "Credential handling":**
  - The master `auth.json` lives only in the runner's store.
  - Each job gets a copy in its per-job `CODEX_HOME`.
  - After the job the runner writes the copy back if the refresh token changed, compare-and-swap against the
    master read at job start.
  - Jobs that share a credential are serialised: one job per credential at a time, as T1 already states. A
    concurrent refresh in two copies would burn the master.
  - A refresh failure (`refresh_token_reused|expired|invalidated` in the error text) marks the credential
    unusable and needs owner re-login.
- **Output schemas** follow the strict subset in finding 4. Contract types generate schemas without `oneOf`: use
  `anyOf` or a tagged object with nullable variant fields. Every property is required and optional means
  nullable. `additionalProperties: false` everywhere.

## Plan impact
- `JobResult.usage`: per job compute the delta of cumulative totals, or sum `last_token_usage` from the rollout.
  Do not take `turn.completed.usage` as per-job usage.
- `LlmProvider.cancel`: SIGTERM to the Codex process (not a shell wrapper). Outcome "cancelled" = rc 143 and no
  terminal event.
- Runner: lean config generated per job; per-job `CODEX_HOME` outside `/tmp`; credential write-back step.
  `rate_limits` from the rollout feed the usage-limit pause.
- Fallback "headless subscription auth impossible": **not triggered**.
