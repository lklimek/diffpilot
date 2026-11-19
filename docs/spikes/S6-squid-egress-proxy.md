# S6 — Squid egress proxy

Date: 2026-11-16. Squid 7.7 (`squid-openssl 7.7-0ubuntu0.26.04.2`, `ubuntu:26.04` pinned by digest), Docker
29.9.2, Compose 5.6.1, Codex 0.167.0, `github-mcp-server` 2.1.0 (tarball SHA-256 `a442f9db…7b2b` re-verified).
Evidence and reusable files: `/data/artifacts/smart-review/2026-11-16/spikes/S6/` — `proxy/` (Dockerfile,
`squid.conf`, `entrypoint.sh`), `compose.yaml` (spike), `compose-snippet.yaml` (WP3-OPS shape), `tests/`,
`logs/` (part 1); `part2/` (`scripts/`, `inputs/`, `logs/`, `jobs/<run>/` with JSONL, `proxy-access.log`,
env and leak checks; sanitised, token scan clean).

## Question
Does Squid as an explicit proxy (allow-all, no SSL-Bump, no CA, no cache) with an SSRF guard work as the
runner's only egress for Codex, its MCP servers and agent commands; long streams; private ranges denied except
MemCan; host-name log; hardening; which source address does MemCan see (O5)?

## Setup
- Networks: `sr-spike-s6-internal` (`internal: true`, 172.30.6.0/24) for clients; `sr-spike-s6-egress`
  (172.30.7.0/24, pinned; proxy fixed at 172.30.7.2). Clients and the Codex container only on the internal one.
- Test-only proxies (production `squid.conf` included unchanged): `-stream` (CONNECT to own stream servers),
  `-ctl` (same + default `read_timeout`), `-dns` (resolver 1.1.1.1, bypassing the LAN rebind filter).
- Part 2: S1a image + apt `cargo`, hardened as S1a/S5 (uid 10001, read-only root, cap-drop ALL); per-job
  `CODEX_HOME` with credential copy and compare-and-swap write-back (S1b; `auth.json unchanged` in every run);
  installation token minted per run and revoked (all 204); MemCan token only via a mounted file → env.

## Results
| # | Item | Result | Evidence |
|---|---|---|---|
| 1 | curl (http/https), `git clone`, `cargo fetch` + `cargo search` via proxy; direct egress fails | **pass** | `logs/item1-tools*.log` |
| 2 | WS + SSE through CONNECT to own servers, 36 min, idle gaps 60 s / 17 min / 18 min | **pass** (2160 s) | `logs/item2-streams.log` |
| 2 | Control with default `read_timeout 15 minutes` | both closed at 961 s (60 + 900 idle) | same |
| 2 | Public echo/Wikimedia streams | carried until the server's own cap (600 s, 901 s, T242767) | same |
| 3 | SSRF guard, 21 cases (LAN, router, metadata incl. `[::ffff:a9fe:a9fe]`, gateway ports, loopback forms, names → 127.0.0.1, manager, CONNECT :80) | **pass**, all `TCP_DENIED/403` | `logs/item3-ssrf*.log` |
| 3 | `nip.io` names, TTL-0 rebinding names (1u.ms, rbndr.us) | **pass**, 172.17.0.1 never contacted | `logs/item3-rebind.log` |
| 4 | MemCan `/health`, `/ui/` via proxy; other port 403; no exception without `SR_MEMCAN_URL` | **pass** | `logs/item4-memcan*.log` |
| 5 | Read-only root, uid 13, CapEff 0, `no-new-privileges`, healthy | **pass**, no fallback | `logs/item5-hardening*.log` |
| P1 | `codex exec` gpt-6.1-sol with only `HTTPS_PROXY`, only `https_proxy`, only `ALL_PROXY` | **pass** each (rc 0, "OK"); none set → fails closed after 158 s ("workspace routing discovery failed", 5 retries) | `part2/jobs/m-*` |
| P1 | Sol/high run: 7 steps, shell + both MCP servers (33 s); endurance run: 37 sequential commands incl. `sleep 420`, 569 s | **pass**, no reconnect/error events; one model tunnel held 568 s across the 7-min idle | `part2/jobs/f1-full`, `l2-long` |
| P2 | Agent shell commands inherit the proxy env; curl, `git clone`, `cargo fetch`, MemCan `/health` from the agent | **pass** | `f1-full/stdout.jsonl` |
| P3 | `github-mcp-server` started by Codex, `pull_request_read` #1040 | **pass** with `env_vars` (below); without proxy vars: `dial tcp: lookup api.github.com … server misbehaving` | `part2/logs/d2-github-mcp.log`, `f1-full` |
| P4 | Codex streamable-HTTP MCP client → MemCan `/mcp` (bearer via `bearer_token_env_var`), `search` limit 1 | **pass** with `http://agentic.lan:8190/mcp`; `host.docker.internal` → 403 from MemCan | `f1-full`, `part2/logs/d3-memcan-host.log`, `f1-traefik.log` |
| P5 | `auth.openai.com` OIDC discovery via CONNECT (no refresh forced) | **pass** (200) | `part2/logs/d1-env-variants.log` |

## Findings
1. **Tools honour the env** (part 2, D1, each spelling alone): curl/git/cargo use `HTTPS_PROXY` or `https_proxy`
   for https; curl also `ALL_PROXY`; for plain http curl ignores uppercase `HTTP_PROXY` (libcurl httpoxy rule),
   so `http_proxy` is required. A tool that ignores the env fails closed (no DNS, no route).
2. **Long streams.** Squid closes a tunnel after `read_timeout` without traffic (default 15 min, measured);
   `read_timeout 60 minutes` keeps idle streams. Codex 0.167 uses HTTPS/SSE only (`codex features list`:
   `responses_websockets` = removed) and keeps one pooled `chatgpt.com` connection across turns and idle tool
   time; the path (incl. home-router NAT) kept it through the 7-min idle command.
3. **SSRF guard.** `dst` ACLs check resolved addresses for HTTP and CONNECT; Squid normalises numeric and
   IPv4-mapped forms. The ipcache clamps TTLs to ≥ `negative_dns_ttl` (1 min) and serves check and connect, so
   TTL-0 rebinding does not flip in between (residual: expiry exactly between them, not reproduced). The MemCan
   exception is resolved address + port. CONNECT only to 443; HTTP to 80, 443, 1025–65535.
4. **MemCan addressing (revised in part 2).** Squid, not the client, resolves the name. MemCan's DNS-rebinding
   guard accepts on `/mcp` only loopback or `MEMCAN_ALLOWED_HOSTS` (now `agentic,agentic.lan,10.26.0.69`), so
   `host.docker.internal`/`172.17.0.1` get 403 there (`/health` passes, which hid it in part 1).
   Recommendation: `SR_MEMCAN_URL=http://agentic.lan:8190/` (MCP at `/mcp`), no `extra_hosts`; after the move,
   a name listed in the new host's `MEMCAN_ALLOWED_HOSTS`. The entrypoint derives the SSRF exception from the
   same variable (here `dst agentic.lan` → 10.26.0.69:8190; the bare LAN IP on other ports stays denied).
5. **Source address (O5).** Traefik: `ClientHost 172.30.7.2` for `/health` and `/mcp` (`RequestHost
   agentic.lan`) — the proxy's pinned egress address, inside the existing `172.16.0.0/12` entry. MemCan on
   another host will see the deployment host's LAN IP (masquerade; inferred).
6. **Log format**: `%ts.%03tu %6tr %>a %Ss/%03>Hs %>st %<st %rm %ru %Sh/%<a "%{User-Agent}>h"` on stdout,
   Docker `local` driver rotation. Tunnels are logged once, at close (a 568 s Codex tunnel appears at its end).
   Codex's own tunnels have no User-Agent (`"-"`); MCP HTTP calls show `codex-mcp-client/0.167.0`,
   `github-mcp-server` shows `Go-http-client/1.1`. Loopback healthchecks are not logged (TCP-only probes were).
7. **Footprint.** 7–12 MiB per idle instance (`docker stats`), squid RSS ≈ 25 MiB; 128 MiB limit.
8. **Codex process**: any one of `HTTPS_PROXY`, `https_proxy`, `ALL_PROXY` suffices for model traffic.
9. **Agent commands** see exactly the Codex process env (S1b finding 9); the proxy vars therefore belong in the
   runner-constructed Codex env, and so does `SR_MEMCAN_TOKEN` (needed by `bearer_token_env_var`) — the agent
   can read it (accepted, T6/T9). Leak checks: neither token in rollout, logs or JSONL (0 files).
10. **Stdio MCP servers** get only `HOME LOGNAME PATH SHELL USER LANG LC_ALL TERM TMPDIR TZ` (Codex
   `rmcp-client` `DEFAULT_ENV_VARS`; an env-dump server saw `HOME PATH` + shell vars) plus `env`/`env_vars`.
   Go honours `HTTPS_PROXY` or `https_proxy` (D2, each alone).
11. **Codex's HTTP MCP client honours the proxy for plain-http URLs** (`POST http://agentic.lan:8190/mcp`
   via Squid, `TCP_MISS/200`); a required server that fails aborts the run before any model call.
12. **Hosts seen** (lean S1a config): `chatgpt.com` (model; plus a ~700 kB download every ~270 s, likely the
   model catalogue), `auth.openai.com` (refresh only; reachable), `api.github.com` (github-mcp-server, curl),
   `github.com`, `index.crates.io`, `static.crates.io`, `crates.io` (cargo search), MemCan host:8190.

## Runner env contract (WP3-RUN)
- Codex process (constructed, `env -i`): `HOME`, `PATH`, `CODEX_HOME`, `HTTPS_PROXY`, `https_proxy`,
  `HTTP_PROXY`, `http_proxy` (all `http://egress-proxy:3128`), `NO_PROXY`/`no_proxy=localhost,127.0.0.1`
  (+ in-compose names reached directly; never the MemCan host), `SR_MEMCAN_TOKEN`. No `ALL_PROXY` needed.
- `[mcp_servers.github]`: `env = { GITHUB_PERSONAL_ACCESS_TOKEN = "<token>" }`,
  `env_vars = ["HTTPS_PROXY", "https_proxy", "NO_PROXY", "no_proxy"]`.
- `[mcp_servers.memcan]`: `url = "<SR_MEMCAN_URL>mcp"`, `bearer_token_env_var = "SR_MEMCAN_TOKEN"`,
  `enabled_tools` per WP3-MEMCAN; no proxy setting (inherits the Codex process env).

## Fallbacks and harness notes
- No plan fallback triggered (read-only root fs works; every tool, Codex and both MCP servers honour the env).
- Item 2 used own stream servers (public services cap at 10/15 min). MemCan URL changed from
  `host.docker.internal` to `agentic.lan` (finding 4).
- Harness: curl honours `NO_PROXY` even with `-x` (fixed with `--noproxy ""`); a stream client counted EOF as
  pass once (fixed; outcome unchanged). The first `f1-full` attempt failed on the MemCan Host guard (kept).

## Proposed ADR
> **ADR: Egress proxy v1.** Squid 7.7 from Ubuntu 26.04 `squid-openssl`, own image pinned by digest, explicit
> proxy on port 3128, no SSL-Bump, no CA, no cache. Policy: deny non-private clients, cache manager, unsafe
> ports; allow `SR_MEMCAN_URL`'s host:port (generated at start); deny CONNECT except 443; deny private,
> loopback, link-local, CGNAT, benchmark, multicast/reserved and IPv6 ULA/link-local/NAT64 destinations; allow
> the rest. `read_timeout 60 minutes`. Log format of finding 6 on stdout, Docker `local` rotation, restricted
> access (T9). Container: read-only root, tmpfs `/run/squid`, uid 13, `cap_drop: ALL`, `no-new-privileges`,
> 128 MiB, HTTP healthcheck. Runner on an `internal: true` network only; proxy also on an egress network with a
> pinned subnet and address. Runner env per "Runner env contract"; stdio MCP servers get the proxy via
> `env_vars`. MemCan is addressed by a name it accepts in `MEMCAN_ALLOWED_HOSTS`, resolved by the proxy.
> Interception, allowlist and cache later remain configuration of the same Squid.

## Plan impact
- WP3-OPS: take `proxy/` and `compose-snippet.yaml`; tests: SSRF table, MemCan allow/deny, log fields,
  healthcheck not logged, runner without proxy fails closed.
- WP3-RUN: env contract above; `env_vars` on every stdio MCP server needing the network; required MCP servers
  fail the job before model use (map to a typed error).
- O5 / "For the owner": allowlist address = `172.30.7.2` (or the pinned subnet) while MemCan is on this host —
  already covered by `172.16.0.0/12`; after the move, the deployment host's LAN IP, and the MemCan name used in
  `SR_MEMCAN_URL` must be in that MemCan's `MEMCAN_ALLOWED_HOSTS`.
- Future allowlist seed: finding 12 (plus `*.oaiusercontent.com`, `ab.chatgpt.com` only with default Codex
  features, S1a).
