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
per-job `CODEX_HOME` root is the host's ext4 loop image (`/srv/smart-review/codex-homes`), not a volume. Networks:
`public` 172.29.73.0/24 (server), `runner` 172.29.71.0/24 (internal and isolated: no route out and no host
address, so the runner reaches only the server's runner listener and the proxy), `egress` 172.29.72.0/24 (proxy).

## First deploy

Requirements: Docker Engine 28.0 or newer (bridge gateway mode `isolated` of the runner network; older engines
ignore the option, and the runner's preflight then refuses to start with exit 78), Docker Compose 2.24.4 or newer (`!override` in the local variant), about 15 GB free
disk for images and build cache, plus the codex-homes image. Check: `docker version -f '{{.Server.Version}}'`.

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

   The runner's start-up check (`deploy/runner/preflight.sh`, exit 78) refuses to start unless
   `/var/lib/smart-review-runner/codex-homes` is a separate ext4 filesystem mounted `nosuid,nodev,noexec`, no
   larger than `SR_CODEX_HOMES_MAX_BYTES` (default 16 GiB) and `SR_CODEX_HOMES_MAX_INODES`. The runner binary
   repeats the same check at its own start (exit 78 too), so it holds even when the container's command is changed.

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

   In the editor set `SR_GITHUB_APP_ID`, `SR_GITHUB_CLIENT_ID`, `SR_GITHUB_CLIENT_SECRET`, `SR_MEMCAN_URL` (the
   full MCP endpoint, e.g. `http://memcan.lan:8190/mcp`), `SR_MEMCAN_TOKEN`, and `SR_RUNNER_CLAUDIUS_REF` (a
   claudius commit SHA pins the agent's skills; recommended); keep the rest unless an address clashes.
   `SR_SECRET_KEY` encrypts stored tokens and cannot be rotated in v1: keep a copy with the backups.
   `sudo /srv/smart-review/app/deploy/check-production-config.sh --env-file /srv/smart-review/.env` must pass.

5. MemCan: the server connects directly, the agents through the egress proxy. Add the host name of
   `SR_MEMCAN_URL` to MemCan's `MEMCAN_ALLOWED_HOSTS`, and both source addresses to its `TRAEFIK_IP_ALLOWLIST`:

   | MemCan runs | server connects from | egress proxy connects from |
   |---|---|---|
   | on this Docker host | `172.29.73.2` (`SR_SERVER_PUBLIC_ADDR`) | `172.29.72.2` (`SR_PROXY_EGRESS_ADDR`) |
   | on another host | this host's LAN address (Docker masquerades) | the same LAN address |

   The proxy resolves the MemCan host name once at start and allows exactly that name, port and address (exit 65
   when the name does not resolve). Check both paths after `sr up -d` (step 7); replace `HOST:PORT`. An answer
   from MemCan itself (JSON "Provide a valid API Token", its `ip` field names the source it saw) means the
   allow-list passed; Traefik's plain `Forbidden` means the source is missing; the proxy's own 403 page (HTML)
   means `SR_MEMCAN_URL` does not match the request.

   ```sh
   sr exec server bash -c 'exec 3<>/dev/tcp/HOST/PORT; printf "GET /mcp HTTP/1.1\r\nHost: HOST:PORT\r\nConnection: close\r\n\r\n" >&3; cat <&3'
   sr exec -u 10002 runner bash -c 'exec 3<>/dev/tcp/172.29.71.3/3128; printf "GET http://HOST:PORT/mcp HTTP/1.1\r\nHost: HOST:PORT\r\nConnection: close\r\n\r\n" >&3; cat <&3'
   ```

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

A backup is three parts taken together with every service stopped: the `server-data` volume (consistent
SQLite), the `runner-state` volume and the codex-homes filesystem. Stopping the runner interrupts a running job;
the job's Codex `CODEX_HOME` then stays in codex-homes, and its `auth.json` may be the only current copy of the
credential (a refresh during the job rotates the single-use refresh token). The runner brings that copy back to
the master at its next start, using the job's record in `runner-state` (`codex-staging/`), so both parts must come
from the same moment. The archives contain the Codex credential: store them like a password (root only,
encrypted off-site), together with `.env`.

```sh
sudo install -d -m 0700 /srv/smart-review/backup
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
sr stop
for v in server-data runner-state; do
  docker run --rm --network none -v "smart-review_$v:/v:ro" -v /srv/smart-review/backup:/backup ubuntu:26.04 \
    tar -C /v --numeric-owner -czpf "/backup/$v-$STAMP.tar.gz" .
done
sudo tar -C /srv/smart-review/codex-homes --numeric-owner -czpf "/srv/smart-review/backup/codex-homes-$STAMP.tar.gz" .
sudo cp /srv/smart-review/.env "/srv/smart-review/backup/env-$STAMP"
sr start
```

Restore all three parts of one `STAMP` (on a new host: steps 1 to 4 and 6 of "First deploy" first, with the
backed-up `.env`). `sr up --no-start` creates missing volumes without starting anything. The runner recovers an
interrupted job's credential at its start.

```sh
STAMP=REPLACE_WITH_THE_BACKUP_STAMP
sr up --no-start
sr stop
for v in server-data runner-state; do
  docker run --rm --network none -v "smart-review_$v:/v" -v /srv/smart-review/backup:/backup:ro ubuntu:26.04 \
    sh -c "find /v -mindepth 1 -delete && tar -C /v --numeric-owner -xzpf /backup/$v-$STAMP.tar.gz"
done
sudo find /srv/smart-review/codex-homes -mindepth 1 -delete
sudo tar -C /srv/smart-review/codex-homes --numeric-owner -xzpf "/srv/smart-review/backup/codex-homes-$STAMP.tar.gz"
sr up -d
sr logs runner | grep -i credential
```

## Exit codes and restarts

All services use `restart: unless-stopped`; `sr ps -a` and `docker inspect -f '{{.State.ExitCode}}'
smart-review-runner-1` show the last exit.

| Exit | Service | Meaning | Action |
|---|---|---|---|
| 70 | runner | The supervisor's job-end sweep could not prove every agent process gone within `SR_SUPERVISOR_SWEEP_DEADLINE_MS`; it ended the container (fail closed, ADR-013). The restart starts clean. | Repeated exits (`docker inspect -f '{{.RestartCount}}'`): read the runner log before the exit; an agent may be resisting the sweep. Stop the runner (`sr stop runner`) if it loops. |
| 71 | runner | The supervisor could not set itself up (capabilities, uids, control socket). | Check that the compose file's `cap_add`, `init` and `ipc` are unchanged and the image is the production image. |
| 78 | runner | Preflight or the runner's own start-up check refused. Either codex-homes is missing, not ext4, mounted without `nosuid,nodev,noexec`, or larger than the limits (or, local stack only, a tmpfs with a non-localhost `SR_PUBLIC_URL`); or the runner network is not isolated (a default route, or a gateway address answers); or `SR_RUNNER_SECRET_FILE` is readable by the agent uid 10002 (owned by it, any `other` bit, or a group bit with group 10002). The log line says which. | Filesystem: run `prepare-codex-homes.sh` again; check `findmnt /srv/smart-review/codex-homes` and `SR_CODEX_HOMES_DIR`. Network: upgrade Docker Engine to 28.0 or newer, keep the runner network's `gateway_mode_*: isolated` options, then `sr down && sr up -d` (networks are created anew). |
| 64 | egress-proxy | `SR_MEMCAN_URL` is not `http(s)://host[:port][/path]`. | Fix `.env`, `sr up -d`. |
| 65 | egress-proxy | The MemCan host name of `SR_MEMCAN_URL` does not resolve (the proxy pins its address at start). | Check DNS from the host (`getent hosts HOST`); the proxy retries with every restart. |

**MemCan moved to another address:** the proxy keeps the address it resolved at its start, so agents keep
connecting to the old one (and get nothing) until `sr restart egress-proxy`. The server resolves on every
connection. Update MemCan's allow-list if the source addresses change (First deploy, step 5).
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
loopback, link-local and metadata ranges denied except MemCan (the configured name, port and the address it had at
the proxy's start); no TLS interception, no CA, no cache. Squid's diagnostics (stderr) are limited to critical and
fatal messages, which carry no request data. Log line (T4b; no URL path, query or header):

```
<time> <ms> <client> <result>/<status> <bytes from client> <bytes to client> <method> <host>:<port> <upstream address>
```

**Retention:** Docker `local` log driver, 50 MB x 5 files for the proxy (`deploy/compose.yaml`,
`egress-proxy.logging`); older lines are discarded. Only users with Docker access can read it. Change the
retention there and run `sr up -d egress-proxy`.

**Tests:** `deploy/egress-proxy/test.sh` (SSRF guard, MemCan exception against a resolver that mixes MemCan's
address into other names, no canary from path, query, headers, PRI requests, broken upstream answers or malformed
URLs in stdout or stderr) and `deploy/test-stack.sh` (the running stack: no host service, public listener or
Internet reachable from the runner except through the proxy; limits); CI runs both on every push.

**Later step: interception and allow-list** (owner decision, plan §16). The research
(`/data/artifacts/smart-review/2026-11-16/research/egress-proxy.md` §3 and `egress-proxy-squid-tested.conf`)
describes it as configuration of the same Squid: `ssl_bump` with a splice list (`chatgpt.com`, `openai.com`)
and bump for the rest, a CA in the proxy (`EGRESS_CA_PEM`) trusted by the runner image, an on-disk cache volume,
full-URL logs and a host allow-list (`dstdomain` / `ssl::server_name` ACL files). It changes the T4b log contract
(full URLs) and needs the runner image rebuilt with the CA; update `deploy/egress-proxy/test.sh` with it.

## Local stack (http://localhost)

The same images on one machine, with the per-job `CODEX_HOME` root on a capped tmpfs instead of the loop image
(`deploy/compose.local.yaml`; no host preparation). Its env file needs `SR_PUBLIC_URL=http://localhost:3000`: the
runner refuses the tmpfs otherwise, and `deploy/check-production-config.sh --env-file FILE --local` checks it.
Sign-in needs the GitHub App callback `http://localhost:3000/auth/github/callback` registered on the App.

The development host's instance (compose project `smart-review-local`) takes its generated secrets from
`~/.config/smart-review-local/stack.env` (mode 0600) and the App and MemCan secrets from
`~/.config/smart-review/secrets.env`; the App key `~/.config/smart-review/github-app.pem` stays readable by the
host user only, so `deploy/dev/compose.host-uid.yaml` runs the server under that uid (`SR_SERVER_UID`/`_GID` in
`stack.env`). Initialize a fresh data volume's ownership before the first server start, as shown below.

```sh
cd /data/git-worktrees/home-ubuntu-git-smart-review-wp3-ops   # or any checkout of the same tree
srl() { docker compose -p smart-review-local --env-file ~/.config/smart-review-local/stack.env \
  --env-file ~/.config/smart-review/secrets.env -f deploy/compose.yaml -f deploy/compose.local.yaml \
  -f deploy/dev/compose.host-uid.yaml "$@"; }
srl ps                 # state and health
srl stop               # stop, keeping containers, networks and volumes
srl start              # start again
srl logs -f --since 10m server runner
srl down               # remove containers and networks; volumes (database, Codex credential) stay
```

For a fresh `server-data` volume, run this once as the host user whose uid/gid are configured in
`compose.host-uid.yaml`, before starting the server. Docker otherwise creates a root-owned directory and SQLite
fails with "unable to open database file". This changes only the volume directory's ownership:

```sh
docker run --rm --network none --user 0:0 \
  -v smart-review-local_server-data:/var/lib/smart-review ubuntu:26.04 \
  chown "$(id -u):$(id -g)" /var/lib/smart-review
```

Codex login (once per runner-state volume): copy an existing login volume to volume, never from the host
`~/.codex`; afterwards the source volume is retired (the refresh token is single-use, one live copy only):

```sh
docker run --rm --network none -v sr-spike-codex-home:/src:ro -v smart-review-local_runner-state:/dst ubuntu:26.04 \
  sh -c 'install -d -o 10001 -g 10001 -m 0700 /dst/credentials /dst/credentials/default &&
         install -o 10001 -g 10001 -m 0600 /src/auth.json /dst/credentials/default/auth.json'
```

First sign-in (owner, once per database): open `http://localhost:3000` in a browser on this host (or through an
SSH tunnel `ssh -L 3000:127.0.0.1:3000 agentic`), choose "Sign in with GitHub", sign in as an allowed login
(`lklimek`), approve the App. The server binds the login to its GitHub id at this first sign-in.

Development without containers: `deploy/dev/stack.sh` (fake GitHub, fake provider, no isolation).
`deploy/dev/real-codex-smoke.sh` with `SR_SMOKE_ISOLATION=supervisor` runs one analysis with the production runner
image under its supervisor behind the egress proxy; `SR_SMOKE_STATE_VOLUME=smart-review-local_runner-state` (local
stack's runner stopped first) reuses the stack's Codex credential instead of a second copy.


### Usage and analysis statistics logs

Each accepted terminal runner result emits one `job token usage` info event with job id, kind and
provider input, cached-input and output counts. Counts are absent when the provider did not report them;
reasoning counts are unavailable in the retained result. Duplicate completion posts do not duplicate it.

Each live-to-terminal analysis transition emits one `analysis statistics` info event: analysis id,
changed lines/files, agent files, plan batches, flows, submitted jobs/repair generations, summed finished
job seconds (`plan_seconds`, `pr_context_seconds`, `flow_seconds`, `ask_seconds`), wall-clock
`total_seconds`, the usage totals/report count, and `initial_estimate_seconds` when known. It includes
all rounds; wall time includes pauses. Cancelled or failed jobs without a finish timestamp contribute
no execution duration. It is a snapshot at the transition: later Ask jobs and late runner completion
after cancellation remain visible in usage summaries but cannot change this single event.

The initial estimate is the first pre-start estimate actually returned to the initiating reader for
that PR head. This small, bounded process-local observation is not persisted; after restart, eviction,
or a start without resolve it is absent, never reconstructed as the original prediction. Counts learned
from diff preparation are also absent if cancellation/failure preceded preparation. Logs contain no
prompts, answers, PR titles or other source text. No migration or stored-document shape changed.
