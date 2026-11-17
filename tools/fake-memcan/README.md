# fake-memcan

Development and test only (never in images): a fake MemCan for the dev stack and the full-stack tests. MCP over
streamable HTTP (rmcp) with the tools the server calls, in MemCan's answer shapes: `_import_records`,
`delete_memory` (MemCan's not-found error for an unknown id), `search_memories` (scoped by `project`). Memories are
kept in memory only.

```sh
FAKE_MEMCAN_TOKEN=<token> cargo run -p fake-memcan -- --listen 127.0.0.1:3910
```

| Flag       | Env                  | Default          | Meaning                                                               |
| ---------- | -------------------- | ---------------- | --------------------------------------------------------------------- |
| `--listen` | `FAKE_MEMCAN_LISTEN` | `127.0.0.1:3910` | Listen address.                                                       |
| `--token`  | `FAKE_MEMCAN_TOKEN`  | —                | Bearer token of every request (pass it through the environment).      |
| `--path`   | `FAKE_MEMCAN_PATH`   | `/mcp`           | Path of the MCP endpoint (`SR_MEMCAN_URL` = `http://<listen><path>`). |

Control and inspection (same bearer token):

| Endpoint            | Behaviour                                                                                         |
| ------------------- | ------------------------------------------------------------------------------------------------- |
| `GET /_fake/state`  | `{mode, memories, deleted, calls, unauthorized}`: stored memories by id, deleted ids, tool calls. |
| `POST /_fake/mode`  | `{"mode": "normal" \| "reject" \| "unavailable" \| "hang"}` or `{"mode": "delay", "ms": N}`.      |
| `POST /_fake/reset` | Forgets memories, calls and mode.                                                                 |

Modes: `reject` answers tool calls with an error result (nothing stored); `unavailable` answers every MCP request
with 503 (no session: MemCan unreachable); `hang` holds MCP requests until the mode changes (at most two minutes);
`delay` carries tool calls out after `ms` milliseconds.

`deploy/dev/stack.sh` starts it with a generated token; `e2e/support/app.ts` (`memcan`) drives it.
