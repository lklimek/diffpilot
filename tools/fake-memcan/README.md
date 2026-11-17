# fake-memcan

Development and test only (never in images): a fake MemCan for the dev stack and the full-stack tests. MCP over
streamable HTTP (rmcp) with the tools the server calls, following MemCan's contract (memcan-server `serve.rs` and
memcan-core `import.rs` at dd00d97): `_import_records` validates every JSONL record (`_collection`, `id`, text
`data`) and answers `imported`/`skipped`/`errors` as MemCan counts them; `delete_memory` answers MemCan's not-found
error for an unknown id; `search_memories` scopes by `user_id`, else `project:<project>`, else `global`, and sets
`has_more` past the limit; unusable arguments and unknown tools are JSON-RPC invalid-params errors. Not modelled:
ranking (memories come in id order), id prefixes in `delete_memory`. Memories are kept in memory only.

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
| `POST /_fake/mode`  | `{"mode": "<mode>"}` (below) or `{"mode": "delay", "ms": N}`.                                     |
| `POST /_fake/reset` | Forgets memories, calls and mode.                                                                 |

Modes: `embedding_failure` is MemCan with a failing embedding provider (imports report every valid record skipped
with an `embedding batch failed` error, searches fail with an internal JSON-RPC error, deletes work);
`synthetic_tool_error` is not a MemCan behaviour: every tool call answers an `isError` result and changes nothing
(the server's "MemCan rejected the call" path); `unavailable` answers every MCP request
with 503 (no session: MemCan unreachable); `hang` holds MCP requests until the mode changes (at most two minutes);
`delay` carries tool calls out after `ms` milliseconds.

`deploy/dev/stack.sh` starts it with a generated token; `e2e/support/app.ts` (`memcan`) drives it.
