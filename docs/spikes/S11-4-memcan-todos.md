# S11-4 — Reading MemCan TODOs from the diffpilot server

Sources: memcan checkout `/home/ubuntu/git/memcan/rs` (`memcan-server/src/serve.rs`, `memcan-core/src/todo.rs`, `ui/api.rs`), diffpilot `crates/server/src/memcan/client.rs`, plan §G. One real read via MCP tools (project `diffpilot`, read-only).

## 1. Interfaces

| ID | Interface | Exposes TODOs | Notes |
|---|---|---|---|
| I1 | MCP `list_todos(project*, status?, owner?, limit?)` | yes | `project` REQUIRED; limit default 100, clamp 1..200; returns `{"todos":[TodoItem],"has_more":bool}`; sorted priority (high first), then created_at, id; NO offset/cursor; status filter is a single value (pending/done/in_progress/blocked/postponed/cancelled), no "all open"; scan hard cap 10 000 rows/project |
| I2 | MCP `export_collection(collection="todos", filter?, limit?, offset?)` | yes | default limit 1000, clamp 1..10000; offset paging; `filter` = raw LanceDB SQL (e.g. `project = 'x' AND status IN ('pending','in_progress','blocked','postponed')`); no ordering guarantee (plain scroll) |
| I3 | MCP `get_todo(todo_id)` | one TODO | full id or unique prefix |
| I4 | MCP `search` with `collections=["todos"]` | yes | semantic, not suitable for listing |
| I5 | `POST /ui/api/mcp` | proxy to I1/I2/I3 | browser session cookie + CSRF only; NOT usable by a server client |
| I6 | Plain REST API | none | no REST for TODOs; everything is MCP over streamable HTTP, `Authorization: Bearer <MEMCAN_API_KEY>` |

## 2. Shapes (verified live)

I1 TodoItem fields: `id` (uuid str), `title`, `description` (str|null), `project`, `priority` (high/medium/low), `status`, `owner` (str|null), `blocked_by` (array of ids), `created_at` (RFC3339), `completed_at` (str|null). **No `updated_at`** (plan §G lists it; wrong).

I2 answer is one text block: JSON `{"count":N,"offset":N,"data":"<JSONL string>"}`; each JSONL line = `{"_collection":"todos","id","collection","data":"<title>\n<description>", "title","description","project","priority","status","owner","blocked_by","created_at","completed_at"}`. Same fields as I1, plus a `data` field that DUPLICATES title+description, so an export page is ~2x the size of an I1 page and double-encoded (JSON in a string). `count` is the line count; there is no `has_more`; paging ends on a short page.

Real read (project diffpilot, 2 TODOs, both pending): I1 = 1 page, `has_more:false`; I2 (limit 2) payload about 3.2 KB, about 1.6 KB per TODO. Redacted example: `{"id":"<uuid>","title":"<..>","description":"<~600 chars>","project":"diffpilot","priority":"high","status":"pending","owner":null,"blocked_by":[],"created_at":"2026-11-16T15:11:02.7+00:00","completed_at":null}`.

## 3. diffpilot client today

| ID | Fact |
|---|---|
| C1 | `server::memcan::client::MemcanClient::connect(&MemcanConfig)` then `.call(tool: &'static str, args: Value) -> Result<String, MemcanError>`: rmcp streamable-HTTP MCP client, bearer from `SR_MEMCAN_TOKEN`/`SR_MEMCAN_TOKEN_FILE`, URL `SR_MEMCAN_URL` |
| C2 | Limits: connect 5 s, call 20 s, answer <= `MAX_ANSWER_BYTES` 256 KiB (over = `InvalidResponse`), no retry |
| C3 | Generic over tool name: `list_todos`/`export_collection` need NO new dependency; only serde_json parsing |
| C4 | Errors: `Unreachable` (network/TLS/auth), `Timeout`, `Rejected`, `InvalidResponse`; `is_unavailable()` distinguishes down. Auth failure surfaces as `Unreachable` (handshake fails), indistinguishable from down except by `detail` |

## 4. Recommended call sequence

1. Per refresh: ONE `MemcanClient::connect`, then per configured project P: `list_todos(project=P, limit=200)` (no status filter). Filter client-side to status in {pending, in_progress, blocked, postponed}. If `has_more` is true, fall back for that project to four calls `list_todos(project=P, status=S, limit=200)` (one per open status) and flag "truncated" if any still reports `has_more`.
   - Why not export_collection: no ordering, 2x payload (duplicate `data`), raw-SQL filter built from user input (injection risk; needs escaping), no benefit once the project list is known. Use it only if the project list is unknown (see 5), with `filter = "status IN ('pending','in_progress','blocked','postponed')"`, limit 100, paging by offset until a short page.
   - Size: about 0.8 KB per TODO in I1 (no `data` dup); 200 TODOs can reach about 160 KB, long descriptions may exceed `MAX_ANSWER_BYTES`. Mitigation: use limit 100 (default) and per-status calls; on `InvalidResponse` retry once with limit 25 or skip descriptions (cannot be excluded server-side; truncate on parse).
2. Do not use `owner` filter by default (owner is mostly null; "no filter for unowned"). Optional: show all owners.

## 5. Project list per user

TODOs are per project string; MemCan has no user concept and no "list projects" tool (`list_collections` lists collections only). So the dashboard needs an explicit per-user setting `memcan_projects: [string]` (the F.6 scope field), default empty or derived from repo names of the user's tracked PRs (only a hint, MemCan project = git remote repo name per user convention, so `lklimek/diffpilot` -> `diffpilot`). Validate names (`^[A-Za-z0-9._-]{1,100}$`), cap count (for example 20), since each is one call. Alternative for discovery: one I2 scan with `filter` and collect distinct `project` (paged, heavy; not recommended on every refresh).

## 6. Field mapping to dashboard item

| ID | MemCan | Dashboard |
|---|---|---|
| M1 | `id` | item id / link key |
| M2 | `title` | title |
| M3 | `description` | detail (truncate to e.g. 500 chars; may contain secrets or markdown, render as plain text) |
| M4 | `project` | group/label |
| M5 | `priority` high/medium/low | rank input |
| M6 | `status` | badge; deterministic fact |
| M7 | `owner` | optional label |
| M8 | `blocked_by` | "blocked by N" |
| M9 | `created_at` | age (no `updated_at`; do not claim "last updated") |
| M10 | `completed_at` | always null for open |

Link: `<SR_MEMCAN_UI_URL>/tasks/<uuid>` where the UI base is `http://agentic.lan:8190/ui` (`/ui/tasks/<uuid>` exists in `memcan-server/src/ui/mod.rs`; UI needs its own login, so the link works only for users signed into MemCan UI). Note: `SR_MEMCAN_UI_URL` must INCLUDE `/ui` (plan §G says `+ /tasks/<uuid>`; define it as the `.../ui` base) and differs from `SR_MEMCAN_URL` (MCP endpoint, not user-facing).

## 7. Failure modes

| ID | Case | Behaviour |
|---|---|---|
| F1 | MemCan down/timeout | `Unreachable`/`Timeout`; keep previous snapshot, mark "not refreshed (HH:MM)" per plan §G; skip remaining projects (`is_unavailable`) |
| F2 | Bad/expired token | handshake fails = `Unreachable`; status should say "check SR_MEMCAN_TOKEN" (message already does) |
| F3 | Unknown project | empty list, no error; UI should show "0 open" and not distinguish typo from none |
| F4 | Over-size answer | `InvalidResponse`; retry smaller limit, else mark project "too large" |
| F5 | `has_more` | per-status fallback, then "truncated" note; never silently drop |
| F6 | Invalid status/limit | `Rejected` (`invalid status ...`) |

## 8. Contradictions with plan §G

1. Tool: plan chooses `export_collection(limit=200)` for all; findings: `list_todos` per project is better (I2 has no project scope without a SQL filter, doubles payload, unordered). Plan's R11-6 fallback becomes the primary.
2. `updated_at` does not exist; fields are `id, title, description, project, priority, status, owner, blocked_by, created_at, completed_at` (+ `data` in export only).
3. "Page size 200 keeps under 256 KiB": true only for typical TODOs (about 0.8 KB, I1) and false for export pages (about 1.6 KB each = 320 KB at 200).
4. Export answer is an envelope with JSONL inside a string, not a bare page.
5. Plan §G does not say how projects are chosen: needs a per-user project list setting (section 5).
6. `SR_MEMCAN_UI_URL` should be the `/ui` base.

## 9. Not verified

Real-size page (only 2 TODOs exist for the project; sizes extrapolated), behaviour on a >100-TODO project, behaviour on a token error (read from source only), whether `SR_MEMCAN_UI_URL` host `agentic.lan:8190` is reachable from the server container, rmcp handling of an exactly-256 KiB answer. No TODO or memory was created or changed.
