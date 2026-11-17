# smart-review runbook

Operating the compose stack of `deploy/compose.yaml`: `server` (web UI and API), `runner` (root supervisor, runner
and agents, ADR-013) and `egress-proxy` (Squid, the runner's only way out). Variables: `.env.example`.
Design background: `docs/implementation-plan.md` §5.4 (threat model), ADR-003 (LLM credential), ADR-013 (runner).

Every block below is meant to be pasted as is into a root shell or a shell of a user in the `docker` group
(marked). Paths assume the checkout in `/srv/smart-review/app` and the configuration in `/srv/smart-review/.env`.
Define the helper once per shell:

```sh
sr() { docker compose --env-file /srv/smart-review/.env -f /srv/smart-review/app/deploy/compose.yaml "$@"; }
```

Volumes (compose project `smart-review`): `smart-review_server-data` (SQLite database `smart-review.sqlite`,
bare mirrors), `smart-review_runner-state` (Codex credential, agent sessions, mirrors, claudius checkout). The
per-job `CODEX_HOME` root is the host's ext4 loop image, not a volume. Networks: `public` 172.29.73.0/24,
`runner` 172.29.71.0/24 (internal), `egress` 172.29.72.0/24.

## First deploy

Requirements: Docker Engine 25 or newer (healthcheck start interval), Docker Compose 2.24.4 or newer (`!override`
in the local variant), about 15 GB free disk for images and build cache, plus the codex-homes image.

1. Checkout (any user with access to `/srv/smart-review`):

   ```sh
   sudo install -d -m 0755 /srv/smart-review
   sudo git clone https://github.com/lklimek/smart-review.git /srv/smart-review/app
   ```

2. Bounded filesystem for the per-job `CODEX_HOME`s (root; creates an 8 GiB ext4 image, an `/etc/fstab` line and
   mounts it; safe to run again, never reformats):

   ```sh
   sudo SR_CODEX_HOMES_SIZE=8G /srv/smart-review/app/deploy/host/prepare-codex-homes.sh
   ```

   The runner's start-up check (`deploy/runner/preflight.sh`) refuses to start unless
   `/var/lib/smart-review-runner/codex-homes` is a separate ext4 filesystem no larger than
   `SR_CODEX_HOMES_MAX_BYTES` (default 16 GiB) and `SR_CODEX_HOMES_MAX_INODES`.

3. GitHub App (github.com → Settings → Developer settings → GitHub Apps → the App):
   - Callback URL, exactly: `https://review.klimek.ws/auth/github/callback` (no wildcard); "Expire user
     authorization tokens" on.
   - Repository permissions: Pull requests read and write; Contents, Checks, Issues, Metadata read-only.
   - Installed on the repositories to review.
   - Generate a private key and place it for the server's uid 10010, read-only:

     ```sh
     sudo install -d -m 0755 /srv/smart-review/secrets
     sudo install -o 10010 -g 10010 -m 0400 ~/Downloads/*.private-key.pem /srv/smart-review/secrets/github-app.pem
     ```

4. Configuration (root; the file holds secrets, mode 0600):

   ```sh
   sudo install -m 0600 /srv/smart-review/app/.env.example /srv/smart-review/.env
   sudo sed -i \
     -e "s|^SR_SECRET_KEY=.*|SR_SECRET_KEY=$(openssl rand -base64 32)|" \
     -e "s|^SR_RUNNER_SECRET=.*|SR_RUNNER_SECRET=$(openssl rand -hex 32)|" \
     -e "s|^SR_PUBLIC_URL=.*|SR_PUBLIC_URL=https://review.klimek.ws|" \
     /srv/smart-review/.env
   sudoedit /srv/smart-review/.env
   ```

   In the editor set `SR_GITHUB_APP_ID`, `SR_GITHUB_CLIENT_ID`, `SR_GITHUB_CLIENT_SECRET`, `SR_MEMCAN_URL`,
   `SR_MEMCAN_TOKEN`; keep the rest unless an address clashes. `SR_SECRET_KEY` encrypts stored tokens and cannot
   be rotated in v1: keep a copy with the backups.

5. MemCan: add the MemCan host name of `SR_MEMCAN_URL` to MemCan's `MEMCAN_ALLOWED_HOSTS`, and the address the
   egress proxy connects from to MemCan's `TRAEFIK_IP_ALLOWLIST`: `172.29.72.2` (`SR_PROXY_EGRESS_ADDR`) when
   MemCan runs on this Docker host, the host's LAN address when it runs elsewhere (Docker masquerades). The
   proxy's private-range guard lets exactly the host and port of `SR_MEMCAN_URL` through.

6. Build the images, one at a time (heavy; check `free -g` first, nothing else compiling):

   ```sh
   sr build egress-proxy && sr build runner && sr build server
   ```

7. Start and check (all three `healthy` within a minute):

   ```sh
   sr up -d
   sr ps
   ```

8. Codex sign-in (once; see "LLM credential"). Then the owner's reverse proxy routes `review.klimek.ws` to
   `127.0.0.1:3000` (`SR_PUBLISH_ADDR`:`SR_HTTP_PORT`); with Docker's userland proxy (the default), requests
   enter the server from the public network's gateway `172.29.73.1`, which `SR_TRUSTED_PROXIES` trusts for
   `X-Forwarded-For`.

9. Sign-in allowlist: the database starts with `lklimek` and `Claudius-Maginificent`. Adding a login (server
   stopped; the helper container needs Internet for `apt`):

   ```sh
   sr stop server
   docker run --rm -v smart-review_server-data:/data ubuntu:26.04 sh -c \
     "apt-get update -qq && apt-get install -y -qq sqlite3 > /dev/null &&
      sqlite3 /data/smart-review.sqlite \"INSERT OR IGNORE INTO tenant_allowed_logins (tenant_id, login) VALUES ('default', 'NEW-LOGIN');\" &&
      chown -R 10010:10010 /data"
   sr start server
   ```

## LLM credential (Codex sign-in, "unusable" recovery)

The runner keeps one Codex credential at `/var/lib/smart-review-runner/credentials/default/auth.json` in the
`runner-state` volume (ADR-003). Sign in inside the running runner container as the runner uid; Codex reaches
the Internet through the egress proxy. Never mount or copy the host's `~/.codex`, and never copy a credential
that another Codex installation still uses (refresh tokens are single-use; both copies would break).

```sh
sr exec -u 10001 runner install -d -m 0700 /var/lib/smart-review-runner/credentials/default
sr exec -it -u 10001 -e CODEX_HOME=/var/lib/smart-review-runner/credentials/default runner codex login --device-auth
sr exec -u 10001 runner test -s /var/lib/smart-review-runner/credentials/default/auth.json && echo "credential present"
```

The device login prints a URL and a code: open the URL, enter the code, approve.

**Unusable credential.** When a refresh fails (token reused, expired or revoked), the runner marks the master
unusable (`auth.json.unusable` next to it, holding the master's digest), the runner log says "the Codex credential
is unusable; sign in again", and Settings shows "Sign-in to the LLM provider is needed". Run the login commands
above; the new credential has a different digest and is used from the next job on, without a restart. If the log
says the state "could not be saved", sign in again before restarting the runner.

**Moving a credential between volumes** (for example, from a test volume that is no longer used), volume to
volume, never through the host:

```sh
docker run --rm --network none -v OLD-VOLUME:/src:ro -v smart-review_runner-state:/dst ubuntu:26.04 sh -c \
  'install -d -o 10001 -g 10001 -m 0700 /dst/credentials/default &&
   install -o 10001 -g 10001 -m 0600 /src/auth.json /dst/credentials/default/auth.json'
```

## Upgrade

```sh
cd /srv/smart-review/app && sudo git pull --ff-only
sr build egress-proxy && sr build runner && sr build server
sr up -d
sr ps
```

`sr up -d` recreates the changed services; a job running on the runner is interrupted and retried by the
server. Before the first release the database schema is a single rewritable `0001_init.sql` without migrations:
when it changes, restore is not possible across the change; start with an empty `server-data` volume.

## Backup and restore

The database and both volumes are copied with the services stopped (consistent SQLite, no job half-way). The
`runner-state` archive contains the Codex credential: store it like a password (root only, encrypted off-site).

```sh
sudo install -d -m 0700 /srv/smart-review/backup
sr stop
for v in server-data runner-state; do
  docker run --rm --network none -v "smart-review_$v:/v:ro" -v /srv/smart-review/backup:/backup ubuntu:26.04 \
    tar -C /v -czf "/backup/$v-$(date -u +%Y%m%dT%H%M%SZ).tar.gz" .
done
sudo cp /srv/smart-review/.env "/srv/smart-review/backup/env-$(date -u +%Y%m%dT%H%M%SZ)"
sr start
```

Restore (replaces the volume content; `STAMP` = the archive's time stamp):

```sh
sr stop
for v in server-data runner-state; do
  docker run --rm --network none -v "smart-review_$v:/v" -v /srv/smart-review/backup:/backup:ro ubuntu:26.04 \
    sh -c "find /v -mindepth 1 -delete && tar -C /v --numeric-owner -xzpf /backup/$v-STAMP.tar.gz"
done
sr start
```

The codex-homes filesystem holds only per-job scratch and needs no backup. The runner discards it at start.

## Exit codes and restarts

All services use `restart: unless-stopped`; `sr ps -a` and `docker inspect -f '{{.State.ExitCode}}'
smart-review-runner-1` show the last exit.

| Exit | Service | Meaning | Action |
|---|---|---|---|
| 70 | runner | The supervisor's job-end sweep could not prove every agent process gone within `SR_SUPERVISOR_SWEEP_DEADLINE_MS`; it ended the container (fail closed, ADR-013). The restart starts clean. | Repeated exits (`docker inspect -f '{{.RestartCount}}'`): read the runner log before the exit; an agent may be resisting the sweep. Stop the runner (`sr stop runner`) if it loops. |
| 71 | runner | The supervisor could not set itself up (capabilities, uids, control socket). | Check that the compose file's `cap_add`, `init` and `ipc` are unchanged and the image is the production image. |
| 78 | runner | Preflight refused: codex-homes is missing, not ext4, or larger than the limits. | Run `prepare-codex-homes.sh` again; check `findmnt /srv/smart-review/codex-homes` and `SR_CODEX_HOMES_DIR`. |
| 64 | egress-proxy | `SR_MEMCAN_URL` is not `http(s)://host[:port][/path]`. | Fix `.env`, `sr up -d`. |
| 1 | server, runner | Configuration or start-up error; the log's last line names the variable. | Fix `.env`, `sr up -d`. |

## Rotating the runner secret

```sh
sudo sed -i "s|^SR_RUNNER_SECRET=.*|SR_RUNNER_SECRET=$(openssl rand -hex 32)|" /srv/smart-review/.env
sr up -d server runner
```

Both services are recreated with the new value; a running job is interrupted and retried. A runner with the old
value is refused ("the server rejected the runner secret").

## Logs

```sh
sr logs -f --since 1h server runner        # application logs (plain text, RUST_LOG level)
sr logs --since 24h egress-proxy | grep -E '^[0-9]{10}\.'   # proxy access log only
```

More detail: set `RUST_LOG=debug` (or `info,runner=debug`) in `.env`, then `sr up -d server runner`. The
supervisor's messages are in the runner's log. All containers log through Docker's `local` driver with size-based
rotation (10 MB x 5 for server and runner).

## Egress proxy

**v1 policy** (threat model T4/T9): explicit proxy, every public destination allowed and logged; private,
loopback, link-local and metadata ranges denied except the MemCan host and port; no TLS interception, no CA, no
cache. Log line (T4b; no URL path, query or header):

```
<time> <ms> <client> <result>/<status> <bytes from client> <bytes to client> <method> <host>:<port> <upstream address>
```

**Retention:** Docker `local` log driver, 50 MB x 5 files for the proxy (`deploy/compose.yaml`,
`egress-proxy.logging`); older lines are discarded. Only users with Docker access can read it. Change the
retention there and run `sr up -d egress-proxy`.

**Tests:** `deploy/egress-proxy/test.sh` (SSRF guard, MemCan exception, host-only log with a canary in path, query
and User-Agent); CI runs it on every push.

**Later step: interception and allow-list** (owner decision, plan §16). The research
(`/data/artifacts/smart-review/2026-11-16/research/egress-proxy.md` §3 and `egress-proxy-squid-tested.conf`)
describes it as configuration of the same Squid: `ssl_bump` with a splice list (`chatgpt.com`, `openai.com`)
and bump for the rest, a CA in the proxy (`EGRESS_CA_PEM`) trusted by the runner image, an on-disk cache volume,
full-URL logs and a host allow-list (`dstdomain` / `ssl::server_name` ACL files). It changes the T4b log contract
(full URLs) and needs the runner image rebuilt with the CA; update `deploy/egress-proxy/test.sh` with it.

## Local stack (http://localhost)

The same images on one machine, with the per-job `CODEX_HOME` root on a capped tmpfs instead of the loop image
(`deploy/compose.local.yaml`; no host preparation). Sign-in needs a GitHub App callback
`http://localhost:3000/auth/github/callback` (the App's LAN development callback).

```sh
cd /srv/smart-review/app   # or any checkout
docker compose --env-file .env -f deploy/compose.yaml -f deploy/compose.local.yaml up -d --build
docker compose --env-file .env -f deploy/compose.yaml -f deploy/compose.local.yaml down        # keeps volumes
```

Development without containers: `deploy/dev/stack.sh` (fake GitHub, fake provider, no isolation).
`deploy/dev/real-codex-smoke.sh` with `SR_SMOKE_ISOLATION=supervisor` runs one analysis with the production runner
image under its supervisor behind the egress proxy.
