# S6 — Squid egress proxy (part 1)

Date: 2026-11-16. Squid 7.7 (`squid-openssl 7.7-0ubuntu0.26.04.2`, `ubuntu:26.04` pinned by digest), Docker
29.9.2, Compose 5.6.1. Evidence and reusable files: `/data/artifacts/smart-review/2026-11-16/spikes/S6/`
(`proxy/` Dockerfile + `squid.conf` + `entrypoint.sh`, `compose.yaml` (spike), `compose-snippet.yaml`
(WP3-OPS shape), `tests/`, `logs/<item>.log`).

## Question
Does Squid as an explicit proxy (allow-all, no SSL-Bump, no CA, no cache) with an SSRF guard work as the
runner's only egress: tools, long streams, private ranges denied except MemCan, host-name log, hardening;
which source address does MemCan see (O5)?

## Setup
- Networks: `sr-spike-s6-internal` (`internal: true`, 172.30.6.0/24) for clients; `sr-spike-s6-egress`
  (172.30.7.0/24, pinned; proxy fixed at 172.30.7.2). The proxy is on both; the client only on the internal one.
- Client env: `HTTP_PROXY`, `HTTPS_PROXY`, lowercase twins, `NO_PROXY=localhost,127.0.0.1`.
- Test-only instances (production `squid.conf` unchanged, included by an overlay): `egress-proxy-stream`
  (allows CONNECT to the own stream servers 172.30.7.10/11:443), `egress-proxy-ctl` (same, plus Squid's default
  `read_timeout 15 minutes`), `egress-proxy-dns` (upstream resolver 1.1.1.1, so the LAN resolver's rebind
  filter does not hide private answers from Squid).

## Results
| # | Item | Result | Evidence |
|---|---|---|---|
| 1 | curl (http + https), `git clone` https, `cargo fetch` (tiny crate, `itoa`) + `cargo search`, all from the internal network through the proxy | **pass** | `logs/item1-tools.log` |
| 1 | Direct egress without proxy: DNS name (`Could not resolve host`), IP literal (`Could not connect`), git | **pass** (fails as required) | same |
| 2 | WebSocket and SSE through CONNECT to own servers, 36 min, fully idle gaps 60 s / 17 min / 18 min | **pass** (tunnels 2160 s, all echoes/events delivered) | `logs/item2-streams.log` |
| 2 | Negative control: same with Squid's default `read_timeout 15 minutes` | both closed at 961 s (= 60 s + 900 s idle) | same |
| 2 | Public endpoints through the production config: wss + SSE `echo.websocket.org`, SSE `stream.wikimedia.org` | carried until the **server** closed them: 600 s (echo: "closed after 10.00 minutes") and 901 s (Wikimedia's 15-min cap, Phabricator T242767; 51.9 MB delivered) | same |
| 3 | SSRF guard: 21 cases (LAN host IP incl. MemCan port, router, 169.254.169.254 incl. `[::ffff:a9fe:a9fe]`, docker0 gateway ports 80/22/11434, egress neighbour, loopback incl. `2130706433`/`127.1`/`[::1]`/`0.0.0.0`, names → 127.0.0.1, cache manager, CONNECT to port 80) | **pass**: all denied `TCP_DENIED/403`, public http/https allowed | `logs/item3-ssrf.log` |
| 3 | Names resolving to private IPs (`nip.io`) and rebinding names (TTL 0: 1u.ms first 1.1.1.1 then 172.17.0.1; rbndr.us random) | **pass**: private answers denied; Squid never contacted 172.17.0.1 (all `HIER_DIRECT/1.1.1.1`) | `logs/item3-rebind.log` |
| 4 | MemCan `GET /health`, `/ui/` via proxy (plain HTTP proxying) → 200; other port on that host → 403; proxy without `SR_MEMCAN_URL` → 403; direct without proxy → unreachable | **pass** | `logs/item4-memcan.log` |
| 5 | `read_only: true` (tmpfs only `/run/squid`, 1 MiB), uid `proxy` (13), `cap_drop: ALL` (CapEff 0), `no-new-privileges`, healthy | **pass**, no fallback | `logs/item5-hardening.log` |

## Findings
1. **Tools honour the env.** curl, git (libcurl) and cargo used the proxy with no tool-specific option; the
   internal network gives neither DNS for external names nor a route, so a tool that ignores the env fails
   closed rather than leaking.
2. **Long streams.** Squid closes a CONNECT tunnel after `read_timeout` without traffic (default 15 min,
   measured); with `read_timeout 60 minutes` idle WebSocket and SSE tunnels survived 17 and 18 min of silence
   and 36 min in total. Public test services cap connection lifetime themselves (10 and 15 min), so they
   cannot prove ≥ 35 min; own servers on the egress network were used instead, reached through a test-only
   overlay that allows CONNECT to their two addresses (production `squid.conf` included unchanged). Not
   covered: NAT idle timeouts of the home router on real Internet paths (part 2 runs real Codex streams; S1a saw
   Codex 0.167 use HTTPS/SSE, no WebSocket).
3. **SSRF guard.** `dst` ACLs are checked against the resolved address for plain HTTP and for CONNECT, and
   Squid normalises numeric and IPv4-mapped forms before the check. Squid's ipcache clamps every answer's TTL
   to at least `negative_dns_ttl` (1 min default; `ipcache_entry::updateTtl`), and the ACL check and the
   connection use that cache, so a TTL-0 rebinding name cannot flip between check and connect; in 8 requests
   over 140 s (and 30 rapid ones) Squid only ever connected to 1.1.1.1. Residual risk: the cache entry expiring
   exactly between check and connect (not reproduced). The LAN resolver here already drops private answers for
   `nip.io` names (not relied on). The MemCan exception is by resolved address + port, so any name resolving to the
   MemCan address on its port is allowed — acceptable, it is the same service. CONNECT is limited to port 443
   and plain HTTP to ports 80, 443, 1025–65535 (Squid's stock `SSL_ports`/`Safe_ports`); the agent tools need
   nothing else.
4. **MemCan addressing.** With an explicit proxy, **Squid resolves the MemCan host name**, not the client.
   Recommendation: `SR_MEMCAN_URL=http://host.docker.internal:8190/` while MemCan runs on the Docker host, with
   `extra_hosts: ["host.docker.internal:host-gateway"]` on the **egress-proxy** service only (the runner cannot
   resolve it and does not need to; the gateway IP form `http://172.17.0.1:8190/` works too but hard-codes the
   bridge). After the move, `SR_MEMCAN_URL` becomes the new LAN name or IP; nothing else changes. The proxy
   entrypoint derives the SSRF exception (`dst` host + port) from the same `SR_MEMCAN_URL`, rejects malformed
   values (exit 64) and adds no exception when it is unset (fake MemCan period).
5. **Source address MemCan sees (O5).** Traefik access log: `ClientHost 172.30.7.2` = the proxy's pinned
   address on the egress network (same host, no NAT; already inside the existing `172.16.0.0/12` allowlist
   entry). If MemCan runs on another host, it sees the **deployment host's LAN IP** (Docker masquerade on the
   egress bridge; inferred, not testable here). The server container, which talks to MemCan directly, should
   sit on the same pinned egress subnet, so one entry (`172.30.7.0/24`, or the host LAN IP) covers both.
6. **Log format** (stdout, Docker `local` driver with size rotation for retention):
   `%ts.%03tu %6tr %>a %Ss/%03>Hs %>st %<st %rm %ru %Sh/%<a "%{User-Agent}>h"` = time, duration ms, client,
   result/status, bytes client→server (exfiltration volume), bytes server→client, method, `host:port` for
   CONNECT or URL without query for HTTP, upstream address, user agent (identifies git/cargo/curl/Codex). A
   tunnel is logged once, when it closes (timestamp = close; duration = lifetime), so a long Codex stream appears
   only at its end. Healthcheck requests from loopback are excluded from the log (a TCP-only probe was logged as
   `error:transaction-end-before-headers` every 30 s, so the healthcheck sends a real HTTP request).
7. **Footprint.** Idle `docker stats` 9–12 MiB per instance (RSS of the squid process ≈ 25 MiB); 128 MiB limit.
   `WARNING: HTTP requires the use of Via` at start is expected (`via off`, `forwarded_for delete`: upstreams
   learn neither the runner's address nor the proxy).

## Fallbacks and harness notes
- No plan fallback triggered (read-only root fs works; every tool honoured the proxy env).
- Public echo services for item 2 → own stream servers (above).
- Harness: curl honours `NO_PROXY` even with `-x`, so the first item-3 run let three loopback cases bypass the
  proxy; fixed with `--noproxy ""` (21/21). `ctl-own-sse` printed `PASS` on EOF (client bug, fixed later); its
  stream was in fact closed at 961 s. `logs/*-final.log` repeat items 1, 3, 4, 5 on the final image.

## Proposed ADR
> **ADR: Egress proxy v1.** Squid 7.7 from Ubuntu 26.04 `squid-openssl`, own image pinned by digest, explicit
> proxy on port 3128 (env `HTTP(S)_PROXY` + lowercase), no SSL-Bump, no CA, no cache (`cache deny all`,
> `cache_mem 0`). Policy: deny non-private-network clients, cache manager, unsafe ports; allow
> `SR_MEMCAN_URL`'s host:port (generated at start); deny CONNECT except 443; deny private, loopback,
> link-local, CGNAT, benchmark, multicast/reserved and IPv6 ULA/link-local/NAT64 destinations; allow the rest.
> `read_timeout 60 minutes` for idle streams. Log: the format of S6 finding 6 on stdout, rotated by the Docker
> `local` driver; restricted access (T9). Container: read-only root fs, tmpfs `/run/squid`, uid 13, `cap_drop:
> ALL`, `no-new-privileges`, 128 MiB, HTTP healthcheck. Networks: runner on an `internal: true` network only;
> proxy also on an egress network with a pinned subnet and address so MemCan's allowlist entry stays stable.
> MemCan is addressed by name through the proxy; the name is mapped on the proxy (`extra_hosts`) when MemCan
> runs on the Docker host. Later steps (interception, allowlist, cache) remain configuration of the same Squid.

## Plan impact
- WP3-OPS: take `proxy/` and `compose-snippet.yaml`; WP3-OPS tests: SSRF table of item 3, MemCan
  allow/deny, log line fields, healthcheck not logged.
- O5 / "For the owner": address for `TRAEFIK_IP_ALLOWLIST` = proxy egress address (`172.30.7.2`, or the
  pinned subnet) while MemCan is on the same host — already covered by `172.16.0.0/12`; after the move, the
  deployment host's LAN IP.
- WP3-RUN: `NO_PROXY` must not contain the MemCan host (it is reached through the proxy).

## Part 2 (pending)
Codex `exec` through the proxy (hosts, SSE stream length, token refresh); Codex passing the proxy env to the
commands it runs; `github-mcp-server` through the proxy. Prepared: compose service `codex` (profile `codex`,
image `smart-review-spike-codex:0.167.0`, volume `sr-spike-codex-home` external, internal network only).
