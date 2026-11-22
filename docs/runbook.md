# diffpilot runbook

Operating the compose stack of `deploy/compose.yaml`: `server` (web UI and API), `runner` (root supervisor, runner
and agents, ADR-013) and `egress-proxy` (Squid, the runner's only way out: all agent network traffic goes through
the proxy; Codex's hosted web search is disabled). Variables: `.env.example`.
Design background: `docs/implementation-plan.md` §5.4 (threat model), ADR-003 (LLM credential), ADR-013 (runner).
Deploying elsewhere (published images, Portainer, every variable; no host specifics): `docs/deployment.md`.

Every block below is meant to be pasted as is into a root shell or a shell of a user in the `docker` group
(marked). Paths assume the checkout in `/srv/diffpilot/app` and the configuration in `/srv/diffpilot/.env`.
Define the helper once per shell:

```sh
sr() { docker compose --env-file /srv/diffpilot/.env -f /srv/diffpilot/app/deploy/compose.yaml "$@"; }
```

Volumes (compose project `diffpilot`): `diffpilot_server-data` (SQLite database `diffpilot.sqlite`,
bare mirrors), `diffpilot_runner-state` (Codex credential, agent sessions, mirrors, claudius checkout), and the
disposable `diffpilot_tmp-*` volumes (section "Temporary volumes"). Networks:
`public` 172.29.73.0/24 (server), `runner` 172.29.71.0/24 (internal and isolated: no route out and no host
address, so the runner reaches only the server's runner listener and the proxy), `egress` 172.29.72.0/24 (proxy).

## First deploy

Requirements: Docker Engine 28.0 or newer (bridge gateway mode `isolated` of the runner network; older engines
ignore the option, and the runner's preflight then refuses to start with exit 78), Docker Compose 2.24.4 or newer (`!override` in the local variant), about 15 GB free
disk for images and build cache, plus the scratch volumes. Check: `docker version -f '{{.Server.Version}}'`.

1. Checkout (any user with access to `/srv/diffpilot`):

   ```sh
   sudo install -d -m 0755 /srv/diffpilot
   sudo git clone https://github.com/lklimek/diffpilot.git /srv/diffpilot/app
   ```

2. Disk (nothing to prepare): compose creates every volume, and nothing is kept in RAM. The job work area holds
   a full-history checkout per job (a large repository such as `dashpay/platform` takes about 1.7 GB) and has no
   size limit, so keep several GB free on Docker's disk (`df -h "$(docker info -f '{{.DockerRootDir}}')"`). Where
   that is the host's root filesystem, a full disk stops the whole host, not only this stack.

3. GitHub App (github.com → Settings → Developer settings → GitHub Apps → the App):
   - Callback URL, exactly: `https://review.klimek.ws/auth/github/callback` (no wildcard); "Expire user
     authorization tokens" on.
   - Repository permissions: Pull requests read and write; Contents, Checks, Issues, Metadata read-only.
   - Installed on the repositories to review.
   - Generate a private key and place it for the server's uid 10010, read-only:

     ```sh
     sudo install -d -m 0755 /srv/diffpilot/secrets
     sudo install -o 10010 -g 10010 -m 0400 ~/Downloads/*.private-key.pem /srv/diffpilot/secrets/github-app.pem
     ```

4. Configuration (root; the file holds secrets, mode 0600):

   ```sh
   sudo install -m 0600 /srv/diffpilot/app/.env.example /srv/diffpilot/.env
   sudo sed -i \
     -e "s|^SR_SECRET_KEY=.*|SR_SECRET_KEY=$(openssl rand -base64 32)|" \
     -e "s|^SR_RUNNER_SECRET=.*|SR_RUNNER_SECRET=$(openssl rand -hex 32)|" \
     -e "s|^SR_PUBLIC_URL=.*|SR_PUBLIC_URL=https://review.klimek.ws|" \
     /srv/diffpilot/.env
   sudoedit /srv/diffpilot/.env
   ```

   In the editor set `SR_ALLOWED_LOGINS` (the GitHub logins allowed to sign in, comma-separated:
   `lklimek,Claudius-Maginificent`), `SR_GITHUB_APP_ID`, `SR_GITHUB_CLIENT_ID`, `SR_GITHUB_CLIENT_SECRET`,
   `SR_MEMCAN_URL` (the full MCP endpoint, e.g. `http://memcan.lan:8190/mcp`), `SR_MEMCAN_TOKEN`, and
   `SR_RUNNER_CLAUDIUS_REF` (a claudius commit SHA pins the agent's skills; recommended); keep the rest unless an
   address clashes.
   `SR_SECRET_KEY` encrypts stored tokens and cannot be rotated in v1: keep a copy with the backups.
   `sudo /srv/diffpilot/app/deploy/check-production-config.sh --env-file /srv/diffpilot/.env` must pass:
   besides development switches it refuses values left from `.env.example`, empty secrets, a runner secret under
   32 characters, a public URL that is not https, `$` interpolation and local or development overrides, naming the
   variable only; an unpinned skills ref (a branch such as `main`) is a warning.

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
   `X-Forwarded-For`. The reverse proxy must append the client address to `X-Forwarded-For` (nginx:
   `proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;`): the failed access-token attempt limit counts
   per client address, and without it all clients share one.

9. Sign-in allowlist: `SR_ALLOWED_LOGINS` in `/srv/diffpilot/.env`, applied at every server start (logins added;
   the others removed with their sessions, GitHub tokens and access tokens). After editing it: `sr up -d server`, then
   `sr logs --since 5m server | grep 'sign-in allowlist set'` shows the logins added and removed.

## Secrets as files (optional)

`deploy/compose.secrets.yaml` mounts `SR_SECRET_KEY`, `SR_RUNNER_SECRET`, `SR_GITHUB_CLIENT_SECRET` and
`SR_MEMCAN_TOKEN` read-only from files in `SR_SECRETS_DIR` (default `/srv/diffpilot/secrets`) and passes
`VAR_FILE` instead of `VAR`, so they do not show in `docker inspect` or Portainer (threat model AR-7). Instead of the
secret lines of First deploy step 4 (root; the runner secret is read by the server's uid 10010 and the runner's
10001, never by the agent's 10002, else the runner exits 78):

```sh
cd /srv/diffpilot/secrets
sudo sh -c 'umask 077; openssl rand -base64 32 > sr-secret-key; openssl rand -hex 32 > sr-runner-secret'
sudo sh -c 'umask 077; cat > sr-github-client-secret'   # paste the value, Enter, Ctrl-D
sudo sh -c 'umask 077; cat > sr-memcan-token'           # paste the value, Enter, Ctrl-D
sudo chown 10010:10010 sr-secret-key sr-github-client-secret sr-memcan-token
sudo chmod 0400 sr-secret-key sr-github-client-secret sr-memcan-token
sudo chown 10001:10010 sr-runner-secret && sudo chmod 0440 sr-runner-secret
sudo sed -i -E 's/^(SR_SECRET_KEY|SR_RUNNER_SECRET|SR_GITHUB_CLIENT_SECRET|SR_MEMCAN_TOKEN)=.*/\1=/' /srv/diffpilot/.env
```

Then add the override to the helper and check with `--secret-files`:

```sh
sr() { docker compose --env-file /srv/diffpilot/.env -f /srv/diffpilot/app/deploy/compose.yaml \
  -f /srv/diffpilot/app/deploy/compose.secrets.yaml "$@"; }
sudo /srv/diffpilot/app/deploy/check-production-config.sh --env-file /srv/diffpilot/.env --secret-files
```

## LLM credential (Codex sign-in, "unusable" recovery)

The runner keeps one Codex credential at `/var/lib/diffpilot-runner/credentials/default/auth.json` in the
`runner-state` volume (ADR-003). Sign in inside the running runner container as the runner uid; Codex reaches
the Internet through the egress proxy. Never mount or copy the host's `~/.codex`, and never copy a credential
that another Codex installation still uses (refresh tokens are single-use; both copies would break).

```sh
sr exec -u 10001 runner install -d -m 0700 /var/lib/diffpilot-runner/credentials/default
sr exec -it -u 10001 -e CODEX_HOME=/var/lib/diffpilot-runner/credentials/default runner codex login --device-auth
sr exec -u 10001 runner test -s /var/lib/diffpilot-runner/credentials/default/auth.json && echo "credential present"
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
docker run --rm --network none -v OLD-VOLUME:/src:ro -v diffpilot_runner-state:/dst ubuntu:26.04 sh -c \
  'install -d -o 10001 -g 10001 -m 0700 /dst/credentials/default &&
   install -o 10001 -g 10001 -m 0600 /src/auth.json /dst/credentials/default/auth.json'
```

## Upgrade

The schema is the numbered files in `crates/db/migrations`, append-only: the server applies new ones at its start,
each in its own transaction (a failing one leaves the database at the previous version), and refuses to start when
an applied one has changed or is unknown to it. `scripts/check-migrations.sh` (CI) keeps applied files unchanged.
Never delete the database to upgrade.

1. Fetch the new version, keep the old commit and images for a rollback, check, build (services keep running):

   ```sh
   cd /srv/diffpilot/app
   git rev-parse HEAD | sudo tee /srv/diffpilot/previous-version   # read by the rollback
   sudo git pull --ff-only
   scripts/check-migrations.sh
   git diff --name-status "$(cat /srv/diffpilot/previous-version)" -- crates/db/migrations   # only A lines; M or D: stop
   for i in server runner egress-proxy; do docker tag "diffpilot-$i:local" "diffpilot-$i:rollback"; done
   sr build egress-proxy && sr build runner && sr build server
   ```

2. Stop, back up (section "Backup and restore", full backup), start the new version; migrations run now:

   ```sh
   # export the analysis statistics first (section "Logs"): the new server container starts with an empty log
   sr stop
   # full backup here
   sr up -d
   sr ps
   sr logs --since 10m server | grep -E 'server stopped|migration' || echo "no migration error"
   ```

3. Verify: all three `healthy`, sign-in works, an existing analysis opens.

**Rollback** (the new version fails, or a migration error in the log): once the new version has applied a new
migration, the old image refuses to start against that database, so a rollback always means restoring the step-2
backup together with the previous images and commit; everything written since the upgrade is lost.

```sh
sr stop
# restore the step-2 backup here (section "Backup and restore")
for i in server runner egress-proxy; do docker tag "diffpilot-$i:rollback" "diffpilot-$i:local"; done
cd /srv/diffpilot/app && sudo git checkout --detach "$(cat /srv/diffpilot/previous-version)"   # back later: sudo git switch -
sr up -d
```

**Binary older than the database:** a server image without a migration the database already holds stops with
`the schema migration failed: migration N was previously applied but is missing in the resolved migrations` (exit
1, restart loop). Never run an older image against an upgraded database: roll back with the matching backup.
`... was previously applied but has been modified` means an applied migration was edited: the build is defective;
roll back and report it, never delete the database.

## Upgrade to v1.3

One optional variable is new; add it to `/srv/diffpilot/.env` to use it (step 1 of "Upgrade"):

| Variable | Required | Default | Meaning |
|---|---|---|---|
| `SR_THEPASTACLAW_STATUS_URL` | no | empty | URL of thepastaclaw's review-system status file, e.g. `https://thepastaclaw.github.io/review-system/data/status.json`. Set: the dashboard's thepastaclaw plug-in reads the file (no credential, revalidated by `ETag`) at most once a minute while someone uses the dashboard in a browser, and notes on the pull requests it lists "thepastaclaw: reviewing (phase, step n of m)" or "thepastaclaw: queued (reason)"; it never changes Ready to merge (ADR-022). The file trails by up to about 20 minutes. The server then needs outbound HTTPS to that host (`thepastaclaw.github.io`). Must be `http://` or `https://`. Empty: off, no request. A bad value stops the server at start (exit 1). |

## Upgrade to v1.2

**Set `SR_ALLOWED_LOGINS` before upgrading** (step 1 of "Upgrade"): the v1.2 server does not start without it, and
from then on it is the sign-in allowlist, applied at every start (ADR-007 addendum). List every login that should
keep access, e.g. `SR_ALLOWED_LOGINS=lklimek,Claudius-Maginificent`; a login left out loses its sessions, stored
GitHub tokens and personal access tokens at the first start. Migration 0016 drops the logins 0001 seeded while
nobody has signed in through them. `deploy/check-production-config.sh --env-file` fails while the variable is empty.

## Upgrade to v1.1

v1.1 adds the "My work" dashboard, the daily refresh, personal access tokens, flags in flow analysis and the Ask
conversation. The upgrade is **in place**: migrations 0003 to 0007 upgrade a v1 database and keep all data. Migration
0007 rebuilds the job tables and copies every row, so v1 jobs, their events and usage survive. Do not delete the
database or the runner's session store. The general procedure and the rollback rules of "Upgrade" apply; this
section adds the v1.1 specifics.

**Both images must be rebuilt.** The flow file format is now version 2 (flags replace questions; ADR-014 addendum).
An old runner image checks flow files against the old grammar and must not run with the new server. A flow job that
was queued before the upgrade carries the old format: the server does not hand it to a runner, releases its claim and
spends an attempt, so it fails after its attempts without an agent run. Analyse that flow again afterwards. Nothing
is corrupted. Finished analyses are not converted: their questions show as flags of unknown level.

1. Check that no job is running. The command reads the database read-only on the host (`sqlite3` 3.27+, as for
   `deploy/backup.sh`); no row means no running job. Queued jobs are fine. If rows appear, wait or cancel the
   analyses in the UI.

   ```sh
   sudo sqlite3 -readonly "$(docker volume inspect -f '{{.Mountpoint}}' diffpilot_server-data)/diffpilot.sqlite" \
     "SELECT kind, state, count(*) FROM jobs WHERE state IN ('claimed', 'running') GROUP BY kind, state;"
   ```

2. Fetch and build as in "Upgrade" step 1 (it tags the old images as `rollback` and builds `runner` and `server`).
   Then stop and copy the database volume to a new volume (volume to volume, nothing is deleted):

   ```sh
   sr stop
   docker volume create diffpilot_server-data-v1
   docker run --rm --network none -v diffpilot_server-data:/src:ro -v diffpilot_server-data-v1:/dst ubuntu:26.04 \
     cp -a /src/. /dst/
   ```

   `deploy/backup.sh` and the full backup in "Backup and restore" are the file-based alternatives. Keep the copy until
   the new version has run for a few days.

3. Add the new variables of the section "New configuration" to `/srv/diffpilot/.env` (`SR_MEMCAN_UI_URL` and
   `SR_PR_HYGIENE_URL` are new; both are optional), then start and check:

   ```sh
   deploy/check-production-config.sh --env-file /srv/diffpilot/.env   # from /srv/diffpilot/app
   sr up -d
   sr ps                                    # all three: healthy
   sr logs --since 5m server | grep -E 'server listening|dashboard scheduler started|schema migration failed'
   ```

   The migrations log nothing on success. A normal first start prints `server listening` for both listeners and
   `dashboard scheduler started`. `the schema migration failed` (exit 1, restart loop) means the image does not
   match the database: roll back. Then sign in and open an existing analysis. The first start can take a few
   seconds longer while it resumes unfinished analyses.

**Rollback.** The old server cannot read the new schema (migrations 0003 to 0007 stay applied). Stop, restore the
volume copy, start the previous images and commit:

```sh
sr stop
docker run --rm --network none -v diffpilot_server-data-v1:/src:ro -v diffpilot_server-data:/dst ubuntu:26.04 \
  sh -c 'find /dst -mindepth 1 -delete && cp -a /src/. /dst/'
for i in server runner egress-proxy; do docker tag "diffpilot-$i:rollback" "diffpilot-$i:local"; done
cd /srv/diffpilot/app && sudo git checkout --detach "$(cat /srv/diffpilot/previous-version)"
sr up -d
```

Everything written since the upgrade (dashboard data, tokens, analyses) is lost. Remove the copy when it is no longer
needed: `docker volume rm diffpilot_server-data-v1`.

### New configuration

Two variables are new since v1; none is required. The scheduler and the dashboard have no other variables: they are
configured per user in Settings.

| Variable | Required | Default | Meaning |
|---|---|---|---|
| `SR_MEMCAN_UI_URL` | no | empty | Base URL of the MemCan web UI **including `/ui`**, e.g. `http://memcan.example.lan:8190/ui`. The dashboard links a MemCan TODO to `<base>/tasks/<uuid>`. Must be `http://` or `https://`; a trailing `/` is removed. Empty: TODOs show without links. A bad value stops the server at start (exit 1). |
| `SR_PR_HYGIENE_URL` | no | empty | Base URL of the PR Hygiene service, e.g. `https://pr-hygiene.services.dash.org`. Set: the dashboard's PR Hygiene plug-in reads the service's public API (no credential) at most once a minute while someone uses the dashboard in a browser, shows its statements on pull requests and holds pull requests it does not list as ready out of Ready to merge (ADR-022). The server then needs outbound HTTPS to that host. Must be `http://` or `https://`. Empty: off, no request. A bad value stops the server at start (exit 1). |

Changed behaviour of an existing variable:

- `SR_TRUSTED_PROXIES` (comma-separated addresses or CIDRs, default empty): it now matters beyond logging. The failed
  access-token attempts are limited per client address (30 a minute). Behind the reverse proxy, with the variable
  empty, all clients share the proxy's address and one limit. Set it to the proxy's address as seen by the container
  (the Docker gateway `172.29.73.1` in `.env.example`) and let the reverse proxy append the client address to
  `X-Forwarded-For` (nginx: `proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;`).
  `deploy/check-production-config.sh` warns when it is empty; the warning does not fail the check.

### First use

Open `https://review.example/` (your `SR_PUBLIC_URL`). The start page is now "My work"; the review start page is at
`/review`. Links with `?pr=` keep working. Everything below is in Settings.

Every Settings option, with when it is used and an example: `docs/settings.md` (in the app: Settings → "Settings help").

1. **Settings → Dashboard.**
   - Accounts (linked logins), repositories (with short names, policy repository and path, release branches, issue
     watching) and owners to list when the search cannot see a login. Nothing is watched until you add repositories
     or owners.
   - MemCan TODOs: on or off, plus extra MemCan projects. The repositories' short names are the default projects.
   - Rules: the ready label, bot authors to skip, lookback days.
   - Automatic refresh: on by default, time of day (default 06:00) and time zone. The browser's zone is stored at your
     first visit to "My work" if none is set; without a zone the schedule does not run. Choose another zone here.
   - Save. A refused value names its field.
2. **Refresh.** The "Refresh" button on "My work" starts a scan (about one minute) and then the ranking. A refresh
   scans GitHub with your sign-in; a ranking uses the owner's Codex quota.
3. **Schedule.** The scheduler checks every minute. It runs once per local day at the set time; a run missed (server
   stopped, user not seen) is caught up once on the same local date, so the first visit after the time normally
   triggers one refresh within about a minute. A manual refresh that succeeded after the scheduled time cancels the
   catch-up. Users run one at a time, each offset by 0 to 9 minutes.
4. **Ranking cost.** The ranking judges at most 48 items per refresh and 120 per user and local day; the rest stay
   provisional with a note. Unchanged items are not judged again.
5. **Efforts** (Settings → LLM, installation-wide, shared by all users): "Dashboard ranking effort" (default low) and
   the Ask effort (default medium; migration 0006 gives an existing selection `medium`). The analysis effort is
   unchanged. A new job reads the current value.
6. **Access tokens** (Settings → Access tokens): for agents that read your dashboard. Scope `read` only, expiry 30, 90
   (default) or 365 days, at most 10 live tokens per user, shown once, stored as a hash. Create and revoke there.
   Tokens never change anything. Format, routes and polling rules: `docs/contracts/agent-api.md`.
7. **"Needs sign-in".** If the scheduled run finds no usable GitHub sign-in, the top bar says "Automatic refresh at
   HH:MM needs you to sign in again" until a later refresh succeeds. Sign in again.
8. **Kept GitHub token pair.** With the schedule on, the server keeps your encrypted GitHub token pair after the last
   session ends, so the run at 06:00 can scan. It keeps it for up to 30 days after your last sign-in (ADR-020
   addendum); then it deletes the pair and the schedule records `needs_sign_in`. "Sign out everywhere", removal from
   the sign-in allow-list, or a rejected refresh deletes the pair at once. Switch the schedule off if you do not want
   the pair kept.

### Operating the dashboard

- **State and failures of a refresh.** The top bar of "My work" shows the last scan, a running ranking, a failure, or
  an automatic run that needs sign-in or failed (its tooltip gives the reason). "What the scan missed" in the side
  panel lists unreachable repositories, items the scan could not read and a MemCan outage (the earlier TODO rows
  stay). A failed scan keeps the previous snapshot. Server log lines (`sr logs server | grep -i dashboard`):
  `dashboard refresh started`, `dashboard snapshot stored`, `dashboard refresh failed; the previous snapshot stays`,
  `dashboard refresh cancelled`, `scheduled dashboard run recorded`, `a scheduled dashboard refresh could not be
  started`, `dashboard judge job failed`. A server restart marks a refresh that was scanning as failed and a ranking
  as interrupted; "Retry ranking" restarts the ranking of a stored snapshot.
- **GitHub rate limits.** The scan paces its searches and batches its detail queries, and retries a secondary limit.
  If your primary limit (5000 requests an hour, shared with the browser and token reads) is exhausted, the refresh
  fails with `github_rate_limited`, and a new Refresh is refused with the reset time until then. Wait for it.
- **Turn the schedule off.** Settings → Dashboard → Automatic refresh, clear "Refresh my dashboard automatically
  every day", save. Manual refresh keeps working.
- **Wipe dashboard data.** There is no route or command for it. Old refreshes are pruned by themselves (the newest 10
  per user, plus the one holding the current snapshot) and judgements of items unseen for a long time are deleted.
  Revoke tokens in Settings. Only the rollback above removes everything.

## Backup and restore

Two kinds: a daily **online backup** (`deploy/backup.sh`, services keep running) and a **full backup** with every
service stopped (before an upgrade, and weekly). Both contain the Codex credential, `.env` and the secrets: store
them like a password (root only, encrypted off-site).

### Online backup (daily)

`deploy/backup.sh` (root; needs `sqlite3` 3.27+ on the host: `apt-get install sqlite3`) writes
`/srv/diffpilot/backup/<UTC stamp>/` (mode 0700, files 0600, `SHA256SUMS`): a consistent snapshot of the
database (`VACUUM INTO` while the server writes; WAL mode), `credentials/` and `sessions/` of `runner-state`, a copy
of `.env` and the secrets directory. It keeps the newest 14 (`SR_BACKUP_KEEP`), prints sizes only, never contents.
Mirrors and the claudius checkout are re-created by the runner. The credential is a copy of the moment: a job
running meanwhile may rotate it, and then the restored one is rejected at its first refresh (sign in again, section
"LLM credential"). `deploy/backup.sh --self-test` backs up and restores a throwaway database under a live writer.

Schedule (root):

```sh
sudo tee /etc/systemd/system/diffpilot-backup.service > /dev/null <<'EOF'
[Unit]
Description=diffpilot online backup
[Service]
Type=oneshot
ExecStart=/srv/diffpilot/app/deploy/backup.sh
EOF
sudo tee /etc/systemd/system/diffpilot-backup.timer > /dev/null <<'EOF'
[Unit]
Description=Daily diffpilot online backup
[Timer]
OnCalendar=*-*-* 03:30:00
RandomizedDelaySec=15m
Persistent=true
[Install]
WantedBy=timers.target
EOF
sudo systemctl daemon-reload && sudo systemctl enable --now diffpilot-backup.timer
systemctl list-timers diffpilot-backup.timer; journalctl -u diffpilot-backup.service -n 20
```

A failed run exits non-zero, leaves only a `.partial-<stamp>` directory (removed by the next run) and never pushes
out complete backups. Check daily (or add `OnFailure=` with your own notification unit to the service); both lines
print nothing when all is well:

```sh
systemctl is-failed --quiet diffpilot-backup.service && echo "the last online backup FAILED (journalctl -u diffpilot-backup)"
sudo find /srv/diffpilot/backup -mindepth 2 -maxdepth 2 -name SHA256SUMS -mmin -1560 | grep -q . || echo "no complete backup in the last 26 h"
```

**Off-site, encrypted** (the backup holds the Codex credential, `SR_SECRET_KEY` and the App key): encrypt on this
host to a key whose private half is not on it, then copy, e.g. with `age` (`apt-get install age`; `age-keygen` on
another machine gives the recipient `age1...`):

```sh
B=/srv/diffpilot/backup
S="$(sudo find "$B" -mindepth 2 -maxdepth 2 -name SHA256SUMS -path "$B/[0-9]*T[0-9]*Z/*" -printf '%h\n' | sort | tail -n 1)"
if [ -z "$S" ]; then echo "no complete online backup in $B" >&2; else S="$(basename "$S")"
  sudo tar -C "$B" -cf - "$S" | age -r age1REPLACE_WITH_RECIPIENT | ssh backup-host "cat > diffpilot/$S.tar.age"; fi
```

Restore an online backup (server and runner stopped; the script refuses otherwise). It checks `SHA256SUMS` and the
database's integrity, unpacks first, then moves the current database (with its `-wal`/`-shm`), `credentials/` and
`sessions/` to `pre-restore-<UTC>/` in the same volume and puts the backed-up ones in place; delete those
directories once the service works. `.env` and the secrets stay as they are (copy them by hand when the host is
new). Test a restore now and then on a spare host or a copy of the volumes.

```sh
sr stop server runner
sudo /srv/diffpilot/app/deploy/backup.sh --restore /srv/diffpilot/backup/REPLACE_WITH_THE_STAMP
sr up -d
sr logs runner | grep -i credential
```

### Full backup (stopped)

A full backup is the `server-data` volume (consistent SQLite) and the `runner-state` volume, taken with every
service stopped; the temporary volumes are never backed up. A graceful stop copies a running job's credential back
to the master first. After a crash of the runner, start it once before the backup: only then has it recovered a
credential that a job refreshed into `tmp-runner-codex-homes` (section "Temporary volumes"). The archives contain
the Codex credential: store them like a password (root only, encrypted off-site), together with `.env`.

```sh
sudo install -d -m 0700 /srv/diffpilot/backup
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
sr stop
for v in server-data runner-state; do
  docker run --rm --network none -v "diffpilot_$v:/v:ro" -v /srv/diffpilot/backup:/backup ubuntu:26.04 \
    tar -C /v --numeric-owner -czpf "/backup/$v-$STAMP.tar.gz" .
done
sudo cp /srv/diffpilot/.env "/srv/diffpilot/backup/env-$STAMP"
sudo tar -C /srv/diffpilot/secrets --numeric-owner -czpf "/srv/diffpilot/backup/secrets-$STAMP.tar.gz" .
sr start
```

Restore both volumes of one `STAMP` (on a new host: steps 1 to 4 and 6 of "First deploy" first, with the
backed-up `.env`). `sr up --no-start` creates missing volumes without starting anything; the temporary volumes
are emptied at start anyway.

```sh
STAMP=REPLACE_WITH_THE_BACKUP_STAMP
sr up --no-start
sr stop
for v in server-data runner-state; do
  docker run --rm --network none -v "diffpilot_$v:/v" -v /srv/diffpilot/backup:/backup:ro ubuntu:26.04 \
    sh -c "find /v -mindepth 1 -delete && tar -C /v --numeric-owner -xzpf /backup/$v-$STAMP.tar.gz"
done
sr up -d
sr logs runner | grep -i credential
```

## Temporary volumes

Owner decision: nothing is kept in RAM, and scratch areas have no size limit for now. The scratch areas are Docker
volumes whose names start with `tmp-` and that carry the label `diffpilot.volume=temporary`:

| Volume | Mounted at | Holds |
|---|---|---|
| `diffpilot_tmp-runner-work` | runner `/work` | per-job checkout, the agent's `HOME` and `TMPDIR` |
| `diffpilot_tmp-runner-codex-homes` | runner `/var/lib/diffpilot-runner/codex-homes` | per-job agent `CODEX_HOME` (a copy of the credential while a job runs) |
| `diffpilot_tmp-runner` | runner `/tmp` | the runner's own temporary files |
| `diffpilot_tmp-server` | server `/tmp` | the server's temporary files |

Their content is disposable: never backed up, emptied at every start (the runner: after bringing a credential
copy of an interrupted job back to the master, before the first job; the server: its entrypoint), and safe to
delete whenever the stack is stopped. The only loss: after a crash, a credential a job had just refreshed lives
only in `tmp-runner-codex-homes` until the runner's next start; deleting it before that means signing in again
(section "LLM credential"). Remove them (they are created anew at the next start):

```sh
sr stop
docker volume ls -q --filter label=diffpilot.volume=temporary --filter label=com.docker.compose.project=diffpilot \
  | xargs -r docker volume rm
sr up -d
```

Free-space reserve: the runner takes no job while the filesystem of `/work` (Docker's data root, shared by all
volumes) has less than `SR_RUNNER_DISK_RESERVE_MIB` free (default 2048). It keeps polling the server, so session
deletions still happen, but jobs stay queued. What you see:
- `sr ps` shows the runner `unhealthy` (marker file `/var/lib/diffpilot-runner/disk-below-reserve`);
- the runner log says `the job scratch filesystem is below its free-space reserve` with `free_mib` and
  `required_mib`, at once and then every hour.

What to do: free space on Docker's disk (`df -h "$(docker info -f '{{.DockerRootDir}}')"`; `docker system df`),
for example by removing the temporary volumes as above. Within one poll after space is back, the runner logs
`... has its free-space reserve again; taking jobs`, and the next healthcheck reports it `healthy`. The reserve
only stops new jobs: a running job can still fill the disk. If a job's copy-back of the Codex credential then
fails (`the agent's auth.json could not be copied back`), the rotated token is lost: sign in again (section "LLM
credential").

`docker volume rm` refuses a volume a container still uses (stopped containers included); then `sr down` first
(containers and networks only, the durable volumes stay). A start that cannot empty a leftover stops with exit 1
(runner: `could not empty ...`; server: `cannot empty /tmp`); remove the temporary volumes as above. A job
directory that start-up recovery quarantined (runner-owned, mode 0700, ADR-013) stays in `tmp-runner-work` with an
error log until removed this way.

## Exit codes and restarts

All services use `restart: unless-stopped`; `sr ps -a` and `docker inspect -f '{{.State.ExitCode}}'
diffpilot-runner-1` show the last exit.

| Exit | Service | Meaning | Action |
|---|---|---|---|
| 70 | runner | The supervisor's job-end sweep could not prove every agent process gone within `SR_SUPERVISOR_SWEEP_DEADLINE_MS`; it ended the container (fail closed, ADR-013). The restart starts clean. | Repeated exits (`docker inspect -f '{{.RestartCount}}'`): read the runner log before the exit; an agent may be resisting the sweep. Stop the runner (`sr stop runner`) if it loops. |
| 71 | runner | The supervisor could not set itself up (capabilities, uids, control socket). | Check that the compose file's `cap_add`, `init` and `ipc` are unchanged and the image is the production image. |
| 78 | runner | Preflight or the runner's own start-up check refused: the runner network is not isolated (a default route, or a gateway address answers), or `SR_RUNNER_SECRET_FILE` is readable by the agent uid 10002 (owned by it, any `other` bit, or a group bit with group 10002). The log line says which. | Network: upgrade Docker Engine to 28.0 or newer, keep the runner network's `gateway_mode_*: isolated` options, then `sr down && sr up -d` (networks are created anew). Secret file: section "Secrets as files". |
| 64 | egress-proxy | `SR_MEMCAN_URL` is not `http(s)://host[:port][/path]`. | Fix `.env`, `sr up -d`. |
| 65 | egress-proxy | The MemCan host name of `SR_MEMCAN_URL` does not resolve (the proxy pins its address at start). | Check DNS from the host (`getent hosts HOST`); the proxy retries with every restart. |
| 1 | server, runner | Configuration or start-up error; the log's last line names the variable. `could not empty ...` (runner) or `cannot empty /tmp` (server): a leftover in a temporary volume could not be removed. | Fix `.env`, `sr up -d`. A leftover: section "Temporary volumes". |
| 1 | server | `the schema migration failed` in the last log line: the image does not match the database (section "Upgrade"). | Roll back (section "Upgrade"); never delete the database. |

**MemCan moved to another address:** the proxy keeps the address it resolved at its start, so agents keep
connecting to the old one (and get nothing) until `sr restart egress-proxy`. The server resolves on every
connection. Update MemCan's allow-list if the source addresses change (First deploy, step 5).

## Rotating the runner secret

```sh
sudo sed -i "s|^SR_RUNNER_SECRET=.*|SR_RUNNER_SECRET=$(openssl rand -hex 32)|" /srv/diffpilot/.env
sr up -d server runner
```

Both services are recreated with the new value; a running job is interrupted and retried. A runner with the old
value is refused ("the server rejected the runner secret").
With secrets as files, replace the file instead (`sudo sh -c 'openssl rand -hex 32 > /srv/diffpilot/secrets/sr-runner-secret'`
keeps its owner and mode) and run `sr restart server runner`.

## Logs

```sh
sr logs -f --since 1h server runner        # application logs (plain text, RUST_LOG level)
sr logs --since 24h egress-proxy | grep -E '^[0-9]{10}\.'   # proxy access log only
```

More detail: set `RUST_LOG=debug` (or `info,runner=debug`) in `.env`, then `sr up -d server runner`. The
supervisor's messages are in the runner's log. All containers log through Docker's `local` driver with size-based
rotation (server 50 MB x 10, runner 10 MB x 5, proxy 50 MB x 5; `deploy/compose.yaml`); a re-created container
(`sr up -d` after a change, an upgrade) starts with an empty log.

**Analysis statistics** (one `analysis statistics` line per finished analysis, section "Usage and analysis
statistics logs"): export them before the server is re-created, and from time to time; the file only grows by new
lines.

```sh
sudo install -d -m 0750 /srv/diffpilot/stats
f=/srv/diffpilot/stats/analysis-statistics.log
sr logs --no-log-prefix --timestamps server | grep -F 'analysis statistics' | sudo tee -a "$f" > /dev/null
sudo sort -u -o "$f" "$f"
```

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
(`/data/artifacts/diffpilot/2026-11-16/research/egress-proxy.md` §3 and `egress-proxy-squid-tested.conf`)
describes it as configuration of the same Squid: `ssl_bump` with a splice list (`chatgpt.com`, `openai.com`)
and bump for the rest, a CA in the proxy (`EGRESS_CA_PEM`) trusted by the runner image, an on-disk cache volume,
full-URL logs and a host allow-list (`dstdomain` / `ssl::server_name` ACL files). It changes the T4b log contract
(full URLs) and needs the runner image rebuilt with the CA; update `deploy/egress-proxy/test.sh` with it.

## Local stack (http://localhost)

The same images and volumes on one machine, published on loopback only (`deploy/compose.local.yaml`). Its env file
sets `SR_PUBLIC_URL=http://localhost:3000`; `deploy/check-production-config.sh --env-file FILE --local` checks it.
Sign-in needs the GitHub App callback `http://localhost:3000/auth/github/callback` registered on the App.

The development host's instance (compose project `diffpilot-local`) takes its generated secrets from
`~/.config/diffpilot-local/stack.env` (mode 0600) and the App and MemCan secrets from
`~/.config/diffpilot/secrets.env`; the App key `~/.config/diffpilot/github-app.pem` stays readable by the
host user only, so `deploy/dev/compose.host-uid.yaml` runs the server under that uid (`SR_SERVER_UID`/`_GID` in
`stack.env`). `stack.env` also holds `SR_ALLOWED_LOGINS=lklimek,Claudius-Maginificent`. Initialize a fresh data
volume's ownership before the first server start, as shown below.

```sh
cd /data/git-worktrees/home-ubuntu-git-diffpilot-wp3-ops   # or any checkout of the same tree
srl() { docker compose -p diffpilot-local --env-file ~/.config/diffpilot-local/stack.env \
  --env-file ~/.config/diffpilot/secrets.env -f deploy/compose.yaml -f deploy/compose.local.yaml \
  -f deploy/dev/compose.host-uid.yaml "$@"; }
srl ps                 # state and health
srl stop               # stop, keeping containers, networks and volumes
srl start              # start again
srl logs -f --since 10m server runner
srl down               # remove containers and networks; volumes (database, Codex credential) stay
```

For a fresh `server-data` volume, run this once as the host user whose uid/gid are configured in
`compose.host-uid.yaml`, before the first `srl up -d`. The host-uid override sets `volume.nocopy: true`
so Docker does not replace this ownership with the image's uid 10010 on first mount. Without the chown,
the fresh directory belongs to root and SQLite fails with "unable to open database file".
Order: create/chown the volume with this one-off container, then start the server with all three Compose
files (including the host-uid override). This changes only the volume directory's ownership:

```sh
docker run --rm --network none --user 0:0 \
  --mount type=volume,source=diffpilot-local_server-data,target=/var/lib/diffpilot,volume-nocopy \
  ubuntu:26.04 chown "$(id -u):$(id -g)" /var/lib/diffpilot
```

Codex login (once per runner-state volume): copy an existing login volume to volume, never from the host
`~/.codex`; afterwards the source volume is retired (the refresh token is single-use, one live copy only):

```sh
docker run --rm --network none -v sr-spike-codex-home:/src:ro -v diffpilot-local_runner-state:/dst ubuntu:26.04 \
  sh -c 'install -d -o 10001 -g 10001 -m 0700 /dst/credentials /dst/credentials/default &&
         install -o 10001 -g 10001 -m 0600 /src/auth.json /dst/credentials/default/auth.json'
```

First sign-in (owner, once per database): open `http://localhost:3000` in a browser on this host (or through an
SSH tunnel `ssh -L 3000:127.0.0.1:3000 agentic`), choose "Sign in with GitHub", sign in as an allowed login
(`lklimek`, from `SR_ALLOWED_LOGINS`), approve the App. The server binds the login to its GitHub id at this first
sign-in.

Development without containers: `deploy/dev/stack.sh` (fake GitHub, fake provider, no isolation).
`deploy/dev/real-codex-smoke.sh` with `SR_SMOKE_ISOLATION=supervisor` runs one analysis with the production runner
image under its supervisor behind the egress proxy; `SR_SMOKE_STATE_VOLUME=diffpilot-local_runner-state` (local
stack's runner stopped first) reuses the stack's Codex credential instead of a second copy.


### Usage and analysis statistics logs

Each accepted terminal runner result emits one `job token usage` info event with job id, kind and
provider input, cached-input and output counts. Counts are absent when the provider did not report them;
reasoning counts are unavailable in the retained result. Duplicate completion posts do not duplicate it.

Each live-to-terminal analysis transition emits one `analysis statistics` info event: analysis id,
changed lines/files, agent files, plan batches, flows, submitted jobs/repair generations, summed finished
job seconds (`plan_seconds`, `pr_context_seconds`, `flow_seconds`, `ask_seconds`), wall-clock
`total_seconds`, the usage totals/report count, `initial_estimate_seconds` when known, and `refinish` (true
when a finished analysis ran again for flows analysed again: its earlier finish logged its own line). It includes
all rounds; wall time includes pauses. Cancelled or failed jobs without a finish timestamp contribute
no execution duration. It is a snapshot at the transition: later Ask jobs and late runner completion
after cancellation remain visible in usage summaries but cannot change this single event. Each time the
driver of an open review ends without admitted flow work (background analysis off and nothing requested, or
the remaining flows failed for good) it emits an `analysis parked` info event with the same fields for the run
so far, without `total_seconds`; an analysis can log it many times before its one `analysis statistics` event.
A park releases the in-memory run facts (an analysis whose head moved may stay parked for good): after a park
the final event carries changed lines/files, agent files and plan batches only when a driver ran again, and
`initial_estimate_seconds` never (the first `analysis parked` event has it).
A parked analysis stays `running` for good unless its flows are all analysed or it is cancelled, also after its
pull request head moved. Server start checks every such analysis for admitted work; with many analyses this can
add seconds to the start (`resuming unfinished analyses`), but it does not grow memory.

The error line `could not store the ended analysis state` means a driver stopped on an error and the database
refused the write of `failed` (full disk, read-only file system): the analysis still reads as planning/running
but no task works on it, and no new driver is started by itself. After the storage problem is fixed, a flow
request, switching background analysis on, Pause followed by Resume, or a server restart starts a driver again.

The initial estimate is the first pre-start estimate actually returned to the initiating reader for
that PR head; it covers plan and PR context only (flows are analysed on request). This small, bounded process-local observation is not persisted; after restart, eviction,
or a start without resolve it is absent, never reconstructed as the original prediction. Counts learned
from diff preparation are also absent if cancellation/failure preceded preparation. Logs contain no
prompts, answers, PR titles or other source text. No migration or stored-document shape changed.

### Installation model choice and pausing

Settings → LLM model changes the model and effort for everyone. Default: Sol 6.1, high; the fixed catalog
comes from ADR-002. Saving affects newly submitted jobs, including remaining jobs of running analyses;
a submitted job retains its spec. The server logs the editor and old/new choice, and the Tour shows
choices actually used.

Pause analysis lets the current job finish, retains its result and the PR session, and prevents further
submissions/claims until Resume. This survives server/runner restarts. A provider limit cannot automatically
resume a user pause; Resume may still leave the analysis waiting for the provider. The current PR head is
checked before resume. Cancel instead discards results and cancels unfinished jobs, keeping analytic and private working PR sessions.
