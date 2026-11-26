# diffpilot runbook

Operating `deploy/compose.yaml`: server, runner, networkless launcher, plan gateway and Squid. The launcher
creates per-payer workers and one-shot dependency fetchers from digest-pinned images. Containers, gateway-only
provider credentials and SG1–SG16 replace the old supervisor/login-volume runtime (ADR-025).

Requirements: Linux Docker Engine 28.0 or newer with isolated internal bridges, Compose with `!override`/`!reset`,
Git, curl, jq, Python 3 and flock. Builds use at most two Cargo jobs. On the shared development host, wrap Docker/Cargo
operations in `flock /data/tmp/diffpilot-heavy.lock` and use the prescribed Cargo wrapper.

Before opening its socket, the launcher checks the Engine version and inspects `SR_WORKER_NETWORK` and
`SR_JUDGE_NETWORK`. Both must exist, use the bridge driver, be internal and set
`com.docker.network.bridge.gateway_mode_ipv4=isolated`. IPv6 must be disabled or set
`com.docker.network.bridge.gateway_mode_ipv6=isolated`. Inspection failures stop startup. If a network has
the wrong mode, stop its jobs and recreate that network from the Compose configuration during maintenance;
changing the YAML alone does not alter an existing network. See [Docker's gateway modes](https://docs.docker.com/engine/network/port-publishing/#gateway-modes).

Production examples use this helper; the Local stack section defines its separate `srl` command:

```sh
sr() { docker compose --env-file /srv/diffpilot/.env -f /srv/diffpilot/app/deploy/compose.yaml "$@"; }
```

## First deploy (empty database)

1. Register a GitHub App with callback `https://YOUR_HOST/auth/github/callback`, repository permissions
   Pull requests read/write and Contents, Checks, Issues, Metadata and Commit statuses read-only. Install it on
   the reviewed repositories.
2. Keep the App private key server-only, uid 10010 mode 0400 in production. Configure `.env` from
   `.env.example`: public URL, allowed logins, App IDs/client secret/key path, MemCan URL/token, encryption key
   (`openssl rand -base64 32`), runner secret (`openssl rand -hex 32`), socket GID and image references.
   Keep `.env` private. Preserve `SR_SECRET_KEY` with backups: changing it invalidates encrypted app data.
3. Build server/runner/Squid/worker/fetch together; prepare the curated claudius context with
   `deploy/containers/prepare-claudius.sh`. Pin worker/fetch by repository digest. The local helper below performs
   these builds and obtains actual digests from a registry with its host port published on 127.0.0.1;
   production should use its release registry.
4. Create the four runtime files described below; run `deploy/check-production-config.sh --env-file FILE`
   as a user able to read those files, or its container-based local helper. Production requires HTTPS.
5. Start `docker compose --env-file FILE -f deploy/compose.yaml up -d --no-build --wait`. No preloaded Codex
   login volume is needed. Sign in with GitHub, then link your plan in Settings as described below.
6. Route the reverse proxy to the loopback published port. It must append `X-Forwarded-For`; configure
   `SR_TRUSTED_PROXIES` for the proxy address. Confirm `/healthz`, Settings, an analysis and private Ask.
7. For GitHub webhooks, follow [the v1.4 cutover](#upgrade-to-v14-and-enable-github-webhooks) after the server is up.

MemCan must allow the server's direct source address and Squid's egress source address. Squid pins the configured
MemCan host/port/address at startup; restart it after a DNS/address change. Every other private destination,
metadata and loopback is denied. Do not add arbitrary LAN exceptions to make a failed job pass.

Gateway and Squid have distinct static addresses on every shared network, preventing automatic allocation from
taking a later static address. When changing subnets, update `SR_GATEWAY_MODEL_ADDR`/`SR_PROXY_WORKER_ADDR` and
`SR_PROXY_EGRESS_ADDR`/`SR_GATEWAY_EGRESS_ADDR` together, plus the runner-interface addresses in `.env.example`.

## Secrets as files (optional)

`deploy/compose.secrets.yaml` mounts `SR_SECRET_KEY`, `SR_RUNNER_SECRET`, `SR_GITHUB_CLIENT_SECRET` and
`SR_MEMCAN_TOKEN` read-only from files in `SR_SECRETS_DIR` (default `/srv/diffpilot/secrets`) and passes
`VAR_FILE` instead of `VAR`, so they do not show in `docker inspect` or Portainer (threat model AR-7). Instead of the
secret lines of First deploy step 4 (root; the runner secret is read by the server's uid 10010 and the runner's
10002; the worker never mounts these service-secret files):

```sh
cd /srv/diffpilot/secrets
sudo sh -c 'umask 077; openssl rand -base64 32 > sr-secret-key; openssl rand -hex 32 > sr-runner-secret'
sudo sh -c 'umask 077; cat > sr-github-client-secret'   # paste the value, Enter, Ctrl-D
sudo sh -c 'umask 077; cat > sr-memcan-token'           # paste the value, Enter, Ctrl-D
sudo chown 10010:10010 sr-secret-key sr-github-client-secret sr-memcan-token
sudo chmod 0400 sr-secret-key sr-github-client-secret sr-memcan-token
sudo chown 10002:10010 sr-runner-secret && sudo chmod 0440 sr-runner-secret
sudo sed -i -E 's/^(SR_SECRET_KEY|SR_RUNNER_SECRET|SR_GITHUB_CLIENT_SECRET|SR_MEMCAN_TOKEN)=.*/\1=/' /srv/diffpilot/.env
```

Then add the override to the helper and check with `--secret-files`:

```sh
sr() { docker compose --env-file /srv/diffpilot/.env -f /srv/diffpilot/app/deploy/compose.yaml \
  -f /srv/diffpilot/app/deploy/compose.secrets.yaml "$@"; }
sudo /srv/diffpilot/app/deploy/check-production-config.sh --env-file /srv/diffpilot/.env --secret-files
```

## Container runtime keys and legacy-volume cleanup

Before starting v1.3, create `gateway-secret-key` with `openssl rand -base64 32` (canonical base64
encoding of exactly 32 bytes). Generate `gateway-credentials-token` and `gateway-keys-token` separately
with `openssl rand -hex 32`: each bearer token must contain at least 32 bytes after whitespace trimming,
and the two scope tokens must differ. Keep the encryption key with its gateway state. Set their
`SR_GATEWAY_*_PATH` variables; all three files require uid 10010, mode 0400 in production. The local host-uid server additionally
reads the credentials token through its host group, using mode 0440 (see Local stack).
Copy only the keys-scope token into `runner-gateway-keys-token`, uid 10002, mode 0400, and set
`SR_RUNNER_GATEWAY_KEYS_TOKEN_PATH`. The two keys-scope files must contain exactly the same bytes.
The runner receives no credentials-scope token. Run the production guard as root so it can compare the files
without printing their contents. It checks these four files even when the other secrets come from `.env`.
For an existing secrets-as-files installation, re-own `sr-runner-secret` to `10002:10010`, mode 0440,
before restarting; the server reads through group 10010 and the runner through owner 10002.
Set `SR_DOCKER_GID` to the host Docker socket's numeric group and pin both worker/fetch image digests.

After the owner has confirmed a successful deployment, stop/remove legacy containers that still reference
the old volumes, then run the following explicitly (replace `diffpilot` for a different Compose project):

```sh
sudo deploy/cleanup-legacy-volumes.sh diffpilot --after-successful-deploy
```

The command is idempotent and prints removed volume names only. It refuses the entire cleanup if any target
is attached to a running or stopped container. It removes only `<project>_runner-state`,
`<project>_tmp-runner-codex-homes`, `<project>_tmp-runner-work` and `<project>_tmp-runner`, never mounts or reads
them, and never revokes upstream credentials. Do not run against another installation or a live local stack.


## LLM credential (Settings linking and relinking)

Each payer opens Settings → Codex account → Link ChatGPT plan. Read the visible warning: linking may sign Codex
out on other CLI machines; isolation from those sessions is not guaranteed. Only the owner completes the device
flow in the browser. There is no API-key mode, credential export/import, CLI login in the runner or host-home
mount. Automatic tests use fake auth only; operators must not run real-account auth experiments.

A terminal credential failure or unknown refresh result shows relink needed; quarantine is deliberate. Relink
through Settings rather than retrying/replaying refresh. The gateway keeps a single dedicated chain, durable
in-flight marker and persist-before-use. Unknown outcomes stay quarantined across restart. Explicit Unlink is
the only action that revokes upstream; worker cleanup and local key revocation do not unlink the account.

Linking and usage pauses are per user. Other users can read shared analyses, but their Ask uses their own
payer container and a private context pack. A first same-payer Ask above 80k context waits for explicit
compaction; its input is billed and completion is persisted before the answer.

## Upgrade

Prepare the new images/configuration without touching the running stack. Save the old commit, image tags and
runtime env; stop new work and let effects finish. Stop runner/launcher first (this stops their managed workers),
then all remaining services. Take the full stopped backup below before any migration. Never use `down -v` on a
live installation. Apply the new image/digest env, run the production guard, then `up -d --no-build --wait`.
Confirm health, preserved sign-in/settings and a new analysis/Ask. Rollback requires the old images/commit and
the matching pre-migration full DB-volume backup; never start old binaries against the migrated database.

## Upgrade to v1.4 and enable GitHub webhooks

1. Run `traefik version` on the proxy host, or run it inside the running Traefik container with `docker exec`.
   Proceed with **3.6.4 or newer** on v3 or **2.11.32 or newer** on v2. Save the current router configuration.
   Keep request-body logging disabled in Traefik and cloudflared, including during troubleshooting.
2. Leave `SR_GITHUB_WEBHOOK_SECRET` and `SR_GITHUB_WEBHOOK_SECRET_FILE` unset for the first v1.4 deploy. This
   preserves hourly watch probes until the cutover. Set `SR_GITHUB_WEBHOOK_URL` to
   `https://webhooks.manago.cloud/diffpilot/github` so Settings shows the URL to enter in GitHub. Remove
   `SR_MAINTAINER_DELEGATES` from the deployment environment. Members now configure their
   [linked logins](settings.md#linked-logins) in Settings to relay decision proposals.
3. Write `/srv/diffpilot/compose.webhook-port.yaml` and add `-f /srv/diffpilot/compose.webhook-port.yaml` to `sr`,
   after the base Compose file and any secrets overlays. Keep the override in subsequent deploys. This appends
   a port mapping to the existing UI mapping.

   ```yaml
   services:
     server:
       ports:
         - "127.0.0.1:3002:3002"
   ```

   Use loopback when Traefik reaches the host through loopback. For remote Traefik, replace `127.0.0.1` with a
   host address only Traefik can reach. If the address is shared and Docker uses its iptables backend, restrict
   the published port to Traefik's source IP in `DOCKER-USER`, matching the original destination address and
   port with `conntrack`. Docker-published ports bypass ordinary ufw rules. See [Docker's firewall documentation](https://docs.docker.com/engine/network/firewall-iptables/).

   Deploy v1.4 using the [upgrade procedure](#upgrade), or apply the override to an already running v1.4 stack
   with `sr up -d --no-build --wait server`. Verify the server is healthy and `sr port server 3002` shows the
   chosen bind address and port.
4. **The owner switches the route after the v1.4 server is deployed.** Prepare the exact Traefik match
   ``Path(`/diffpilot/github`)`` with the existing host matcher. Set an explicit router `priority` greater than
   the broader UI router's effective priority, for example `100` if the UI priority is lower. Configure
   `ReplacePath` to `/webhooks/github` and the service to `http://<host>:3002`, using the address Traefik reaches.
   The upstream URL is `http://<host>:3002/webhooks/github`.

   In one maintenance window, set `SR_GITHUB_WEBHOOK_SECRET` to match the App (at least 32 characters, or use
   `openssl rand -hex 32` for a new secret). For files, set `SR_GITHUB_WEBHOOK_SECRET_FILE` and use the
   [webhook secrets overlay](deployment.md#github-webhook-secrets-as-files). Run
   `sr up -d --no-build --wait --force-recreate server` and immediately switch the Traefik route. Once the
   server starts with the secret, watched PRs in repositories it considers live skip hourly probes. Until the
   route is switched and GitHub delivers events, use **Read GitHub again** for those PRs. If the cutover fails,
   roll back the secret and recreate the server to restore hourly probes.

   In the Traefik dashboard or `/api/http/routers` on its admin endpoint, confirm the active rule, priority,
   middleware and service. Check the service's upstream address in `/api/http/services`.
5. Send a POST with a well-formed invalid signature through Traefik's tunnel-facing entry point. Run the check
   from the trusted network, resolving the webhook hostname to Traefik directly. Use the Host expected by the
   router and, if necessary, the entry point's CA with `--cacert`.

   ```sh
   traefik_ip='<traefik-ip>'
   webhook_host='webhooks.manago.cloud'
   delivery_id=$(python3 -c 'import uuid; print(uuid.uuid4())')
   bad_signature=$(printf '%064d' 0)
   curl -sS -o /dev/null -w '%{http_code}\n' \
     --resolve "$webhook_host:443:$traefik_ip" \
     -H "Host: $webhook_host" \
     -H 'Content-Type: application/json' \
     -H 'X-GitHub-Event: ping' \
     -H "X-GitHub-Delivery: $delivery_id" \
     -H "X-Hub-Signature-256: sha256=$bad_signature" \
     --data-binary '{}' "https://$webhook_host/diffpilot/github"
   ```

   **401** means the request reached the webhook receiver and failed signature verification. The server creates
   no delivery receipt and triggers no work. **404** indicates a route to the UI port or a path mismatch. Check
   the router match, priority, rewrite and upstream port before continuing.

6. In the GitHub App, grant read access to **Commit statuses** and subscribe to the
   [listed events](deployment.md#4-github-app). Have installation owners accept the new permission. Set the
   webhook URL, enter the matching secret and save with **Active** and SSL verification enabled. GitHub sends
   a fresh `ping` when the App webhook is first created. Find it under **Advanced → Recent deliveries** and
   click **Redeliver**. Expect HTTP **200**. The server logs that it received the ping or deduplicated the delivery.
   This checks the full path through the tunnel and Traefik with a GitHub signature.

   GitHub can redeliver only the last three days of deliveries. For an existing App with no recent `ping`, check
   **Advanced → Recent deliveries** after saving the webhook settings. If no fresh ping appears, redeliver a
   recent successful event and check for a **2xx**, or wait for the next subscribed event and check its response
   there. GitHub documents [when it sends a ping](https://docs.github.com/en/webhooks/webhook-events-and-payloads#ping)
   and [how to redeliver App webhooks](https://docs.github.com/en/webhooks/testing-and-troubleshooting-webhooks/redelivering-webhooks#redelivering-github-app-webhooks).

**Rollback.** Unset `SR_GITHUB_WEBHOOK_SECRET` and `SR_GITHUB_WEBHOOK_SECRET_FILE`, remove the port override
from `sr`, and run `sr up -d --no-build --wait --force-recreate server`. This restores hourly watch probes and
unpublishes 3002. Disable the Traefik webhook router and deactivate the webhook in the GitHub App. If rolling
back the application too, restore the matching pre-migration backup as described in [Upgrade](#upgrade). Use
Refresh or the nightly scan while webhooks are disabled.

### Rotate the GitHub webhook secret

1. Set `SR_GITHUB_WEBHOOK_SECRET_PREVIOUS` to the old secret and `SR_GITHUB_WEBHOOK_SECRET` to the new one. For
   file-based secrets, put the old value in the `previous` file and the new value in the `current` file, with the
   [matching `_FILE` variables](deployment.md#github-webhook-secrets-as-files). Save the existing `RUST_LOG`
   setting and set `RUST_LOG=info,server::webhooks::receive=debug` in `/srv/diffpilot/.env`. Recreate the server
   with `sr up -d --no-build --wait --force-recreate server`.
2. Replace the webhook secret in the GitHub App settings and save. Read the server's Compose logs with
   `sr logs --since 10m server | grep -F 'GitHub webhook signature verified'`. Wait for a delivery that shows
   `secret=current` (the new secret). Older lines with `secret=previous` show the old secret was used.
3. Remove `SR_GITHUB_WEBHOOK_SECRET_PREVIOUS` or `SR_GITHUB_WEBHOOK_SECRET_PREVIOUS_FILE` and its previous secret
   file. Restore `RUST_LOG` in `.env` (remove the setting if it was unset) and run the recreate command again.

### Refused or missed GitHub deliveries

- The receiver verifies `X-Hub-Signature-256` over the original body and checks the App target before processing
  any delivery. Source IPs and WAF rules provide no authentication. For 401, check the shared secret and App target.
- A 404 can indicate a path mismatch or routing to the UI listener. A 503 can indicate a missing secret, a busy
  receiver or a timeout. The listener can also refuse TCP connections at its connection limit.
- Before the draining 404 handler shipped, the old UI route could reset connections during upload. About 40% of
  deliveries in the observed pre-v1.4 setup ended in 502 at Traefik. The nightly scan and Refresh recover the facts.
- GitHub does not retry failed deliveries automatically and diffpilot runs no redelivery sweep. To retry a
  delivery from the last three days, open GitHub App **Edit → Advanced → Recent deliveries**, select the
  delivery and click **Redeliver**.
  Refresh or the nightly scan recover My work facts. For a watched PR in a live repository, wait for its next
  event or use **Read GitHub again** in Comments.
  See [GitHub's redelivery instructions](https://docs.github.com/en/webhooks/testing-and-troubleshooting-webhooks/redelivering-webhooks).

To read the counters, set `DP_READ_TOKEN` in the environment to a diffpilot personal access token with `read`
scope. Use the UI/API base URL (`SR_PUBLIC_URL`) and run with shell tracing disabled. Pass the bearer header on
standard input so the token stays out of the `curl` arguments and output.

```sh
diffpilot_url='https://review.example.com'
printf 'Authorization: Bearer %s\n' "${DP_READ_TOKEN:?Set the diffpilot read token in the environment}" |
  curl -fsS --header @- "$diffpilot_url/api/v1/settings/github-webhook" |
  jq -e '{configured, last_delivery_at, dropped_deliveries, refused_connections}'
```

With the webhook enabled, expect `configured: true` and a recent `last_delivery_at` after a fresh signed delivery.
The timestamp covers stored receipts and recent untracked drops. Both counters reset on restart. `dropped_deliveries`
counts untracked deliveries over at most the last 14 days. `refused_connections` counts TCP refusals at the
listener's connection limit and stays at zero during normal operation below that limit.

## Upgrade to v1.3

**Container cut-over:** migration 0030 cleans existing analyses and review-loop data in place, including owner
decisions, pending or claimed decision effects, queued notifications and queued MemCan feedback writes. Settings,
sign-ins and stored GitHub authentication stay; see the [table inventory M01–M55](contracts/containers.md#persistence-and-cut-over).
Before deploying, let pending decision effects finish or accept that they will be dropped without execution.
Keep the full step-2 volume backup: restoring it with the previous images and commit is the only rollback path.
After startup, every user must relink Codex before jobs can run. Watched PRs rebuild their review-loop data on the
next probe; automatic assessments use quota and require Codex to be linked again. At step 3 of "Upgrade", check
sign-in and rebuild an analysis instead of opening an old one.

Two optional variables are new; add them to `/srv/diffpilot/.env` to use them (step 1 of "Upgrade"):

| Variable | Required | Default | Meaning |
|---|---|---|---|
| `SR_THEPASTACLAW_STATUS_URL` | no | empty | URL of thepastaclaw's review-system status file, e.g. `https://thepastaclaw.github.io/review-system/data/status.json`. Set: the dashboard's thepastaclaw plug-in reads the file (no credential, revalidated by `ETag`) at most once a minute while someone uses the dashboard in a browser or any user watches a pull request, and notes on the pull requests it lists "thepastaclaw: reviewing (phase, step n of m)" or "thepastaclaw: queued (reason)"; it never changes Ready to merge (ADR-022). The file trails by up to about 20 minutes. The server then needs outbound HTTPS to that host (`thepastaclaw.github.io`). Must be `http://` or `https://`. Empty: off, no request. A bad value stops the server at start (exit 1). |
| `SR_NOTIFY_DENY_PRIVATE` | no | `0` | `1`: users' notification channels (ntfy, webhook) and agents' push targets may not reach private or unique-local addresses; `0`: they may (Home Assistant or a self-hosted ntfy on the LAN). Loopback, link-local (`169.254.0.0/16`, `fe80::/10`), unspecified, multicast, broadcast and reserved addresses (also in IPv6 forms such as NAT64 or 6to4) and Teredo addresses are refused with either value, in the URL and after resolving a host name; notifications connect directly, never through a proxy from the environment. These hosts are outbound destinations chosen by users and agents, not by the operator: with `0` the server sends to any other `http`/`https` host they enter. A bad value stops the server at start (exit 1). |

**Notifications** (ADR-023 decision 11). Each delivery is tried 3 times (now, after about 1 and 5 minutes, 10 s
each); a channel or push target whose last delivery failed shows "Failing since …" with the error in Settings →
Notifications (agents' targets in a PR's decision log). The server logs one `warn` "a notification target started
failing" when a target starts failing and one `info` "a failing notification target delivers again" when it
recovers, never per attempt; answers of the targets are not logged or shown. At most 30 messages an hour go to one
target (a channel counted per user and kind, a push target per token; the count is stored, so a restart or removing
and saving the channel does not reset it); more wait for the next free slot, and a Test press over the limit answers
"try again later" without marking the channel failing. Events wait for their delivery however long the server was
down. Agents' push targets receive nothing once their token is revoked or expired or its user leaves
`SR_ALLOWED_LOGINS`, and nothing while the agent holds its feed stream. Changing a channel's or push target's
kind or the scheme, host or port of its URL needs its secret entered again (or removed). Secrets of channels and
push targets are encrypted under `SR_SECRET_KEY`; after replacing that key they cannot be decrypted: the target
fails with "the stored secret could not be decrypted; set it again" until its owner saves the secret anew.

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

2. Prepare the new images as in "Upgrade", retaining the old image references. For this historical v1-to-v1.1
   migration, stop and copy the database volume to a new volume (nothing is deleted):

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
   sr ps                                    # check each configured service's health
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
| `SR_PR_HYGIENE_URL` | no | empty | Base URL of the PR Hygiene service, e.g. `https://pr-hygiene.services.dash.org`. Set: the dashboard's PR Hygiene plug-in reads the service's public API (no credential) at most once a minute while someone uses the dashboard in a browser or any user watches a pull request, shows its statements on pull requests and holds pull requests it does not list as ready out of Ready to merge (ADR-022). The server then needs outbound HTTPS to that host. Must be `http://` or `https://`. Empty: off, no request. A bad value stops the server at start (exit 1). |

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

### Online database snapshot

`deploy/backup.sh` writes a consistent SQLite `VACUUM INTO` snapshot, configured env and secrets, plus
`SHA256SUMS`; it retains 14 complete snapshots by default. It does **not** back up gateway state, thread volumes
or assessment snapshot files. Its `--self-test` uses a throwaway database with a live writer. Schedule it as root
(or an equivalently restricted service account), monitor its exit status and keep backups private/encrypted.
`--restore DIR` requires server/runner stopped and restores only the database, retaining the replaced DB/WAL.
Use a matching encryption key/config; restoring a DB alone can leave missing runtime files and requires fresh
threads or replay from a context pack.

### Full stopped backup and restore

1. Record compose project, code commit, image references and all env-file names. `sr stop runner launcher`, then
   `sr stop` for the remaining services. Verify no container labelled `dp.managed=PROJECT` is running; do not
   snapshot an active worker or gateway chain.
2. Archive the entire `PROJECT_server-data` volume (DB/WAL plus non-secret frozen assessment snapshots),
   `PROJECT_gateway-data` (encrypted tokens/generations/in-flight markers), `PROJECT_runtime-state`, and each
   labelled per-payer `PROJECT-pr-*-work` volume. Keep gateway state and its own encryption key together.
   Do not archive `launcher-socket`, `tmp-server`, C1 mirrors or C2 dependency caches; those are disposable.
3. Separately archive all configuration, runtime secret files and the App key, preserving numeric owners/modes.
   Backups contain credentials: mode 0700 directories, 0600 archives, encrypt before leaving the host.
4. Restore into empty volumes with the **same names** while all services/workers remain stopped. Preserve
   numeric ownership. Restore config/keys before starting. Never run an original and restored gateway chain
   concurrently, or reuse an old refresh generation after newer use: quarantine/relink if outcome is unknown.
5. Run the guard; start all services from the matching release. Launcher reconciliation and local key revoke
   settle runtime objects. Check DB integrity/foreign keys, Settings, a new job and same-payer Ask. Missing
   thread volumes fall back to a context pack. Restored snapshots must match the restored DB.

A no-sudo volume archive on the dev host uses a one-off root container (operator action, after stopping):

```sh
backup=/data/private-backups/diffpilot-local/$(date -u +%Y%m%dT%H%M%SZ)
install -d -m 0700 "$backup"
docker run --rm --network none --user 0:0 --entrypoint bash \
  --mount type=volume,source=diffpilot-local_server-data,target=/source,readonly \
  --mount "type=bind,source=$backup,target=/backup" diffpilot-local-server:local \
  -ec 'umask 077; tar --numeric-owner -C /source -czf /backup/server-data.tar.gz .'
```

Repeat with each required stopped volume and a distinct archive name. To restore, mount an empty destination
read-write and the archive directory read-only, then `tar --numeric-owner -C /destination -xzf /backup/NAME`.
Do not inspect or publish archive contents. Test restore in a separate project before discarding the original.

## Temporary volumes and disk pressure

Only `PROJECT_tmp-server` is a disposable Compose scratch volume. Remove it while server is stopped if startup
cannot empty `/tmp`. Worker `/run/dp`, `/tmp`, CS2 home and CS2 Codex home are bounded tmpfs and disappear on
stop. CS1 work volumes contain persistent thread history; removing one forces context-pack recovery. C1 and C2
are caches, never credentials. Gateway-data and server-data are durable and must not be pruned.

The runner's disk reserve stops new work below `SR_RUNNER_DISK_RESERVE_MIB` (default 2048 MiB), but is not a
running-job quota. Monitor Docker disk usage and free inodes. Stop jobs before cleanup; never run broad Docker
volume prune on this shared host. Legacy volumes have a dedicated guarded cleanup command below.

## Exit and recovery

On runner/launcher restart, reconciliation stops managed workers and locally revokes stale keys before jobs
resume. Graceful launcher shutdown stops managed workers; worker volumes remain. A missing thread volume uses
a context pack. Gateway unknown refresh outcomes remain quarantined across restart; Settings relink is required.
Inspect component logs and health status; do not reintroduce the removed supervisor flags or credential files.

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
launcher and gateway have separate service logs. All containers log through Docker's `local` driver with size-based
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

Only the owner/coordinator performs these commands on the dev host. The existing project is `diffpilot-local`;
workers/tests must never inspect its secrets, mount its volumes or run its Compose commands. Automated acceptance
uses `deploy/dev/test-local-containers.sh` with unique `v13w7-local-*` names/ports, synthetic secrets and a fake
gateway. It removes every test container, network, volume and registry on exit.

The helper prepares files/images without starting or stopping the application. Its four subcommands are:

| ID | Command | Result |
|---|---|---|
| L1 | `prepare` | Build server (with gateway), runner/launcher, Squid, worker and fetch; start/reuse a local registry; save actual worker/fetch digest refs. |
| L2 | `secrets DIR` | Create only absent NEW runtime secrets using a root container; no sudo or existing-secret read/copy/rotation. |
| L3 | `env FILE` | Exclusively create a non-secret env file containing image refs, Docker GID, host uid/gid and runtime secret paths. |
| L4 | `check ENV... RUNTIME_ENV` | Run the production guard in `--local` mode, same env order as Compose, inside a networkless root container. It validates explicit files without printing secret values; no engine socket mounted. |

`SR_LOCAL_STATE_DIR` defaults to `~/.local/state/diffpilot-containers`. The helper uses a state lock; the caller
also holds the host heavy lock. `SR_LOCAL_REGISTRY_PORT` defaults to 15000; the registry host port is published
on 127.0.0.1. New registries use a dedicated NAT bridge with inter-container communication disabled and no
other containers, a digest-pinned image, a read-only root, dropped capabilities and no-new-privileges; data
is in the `<registry-name>-data` volume.
The host can push via `localhost:PORT`; this is unauthenticated host access, not a registry authentication policy.
`prepare` validates the network and hardening on reuse. A running legacy `registry:2.8.3` on the default bridge
with the expected local label and loopback host binding is kept with a one-line notice; its bridge exposure
and tag pin remain until the owner schedules replacement. The helper never stops or replaces it automatically.
`SR_LOCAL_REGISTRY_IMAGE` overrides must include a SHA-256 digest. This registry bridge is not Docker
`--internal`: Docker does not publish host ports for a container attached only to an internal network.
Use a directory dedicated to the four new gateway files, separate from existing App/server secrets. The server
runs under host uid/gid via `compose.host-uid.yaml`; this host's Docker socket GID is 1000.

```sh
export SR_LOCAL_STATE_DIR=/data/local/diffpilot-container-prep
export SR_DOCKER_GID=1000 SR_SERVER_UID=$(id -u) SR_SERVER_GID=$(id -g)
lcp() { flock /data/tmp/diffpilot-heavy.lock deploy/dev/local-containers.sh "$@"; }
lcp prepare
lcp secrets /data/local/diffpilot-runtime-secrets
lcp env /data/local/diffpilot-runtime.env
lcp check ~/.config/diffpilot-local/stack.env ~/.config/diffpilot/secrets.env /data/local/diffpilot-runtime.env
```

The helper generates gateway encryption key = base64 of 32 random bytes, distinct 32-byte hex credentials/keys
bearers, and a runner copy of the newly created keys bearer. Gateway key/keys files are `10010:10010 0400`,
credentials `10010:<host-gid> 0440`, runner keys `10002:10002 0400`. Reruns leave existing bytes and metadata
untouched. A partial keys pair fails without opening it: the operator must restore the matching pair from its
private backup or explicitly retire/recreate both files while services are stopped. Never print a key or copy
old CLI credentials. If config files live elsewhere, pass their explicit paths to `check`; the helper does not
search the owner's configuration directories.

Before cut-over, let pending effects finish, stop runner/launcher then all services, and back up the entire
`diffpilot-local_server-data` volume using the stopped archive procedure above. On a legacy installation the
launcher service may not exist yet: stop the old runner first. Preserve old images/commit and old volumes for
rollback. Add the runtime env as the **last** env file:

```sh
srl() { docker compose -p diffpilot-local \
  --env-file ~/.config/diffpilot-local/stack.env --env-file ~/.config/diffpilot/secrets.env \
  --env-file /data/local/diffpilot-runtime.env \
  -f deploy/compose.yaml -f deploy/compose.local.yaml -f deploy/dev/compose.host-uid.yaml "$@"; }
```

0030 upgrades the existing DB atomically in place: settings, sign-ins/GitHub auth, users, tenants, channels,
PATs/webhooks, watches and snoozes survive; analysis/runtime/review decisions/effects are cleared. See
[the keep/clean inventory](contracts/containers.md#persistence-and-cut-over). W10 export/import is dropped.
Old Codex credentials are cleared; every payer relinks in Settings. On a fresh empty DB, initialize only its
new volume directory ownership before first start; existing DB volumes must retain their ownership:

```sh
# Fresh volume only; do not run recursively on an existing installation.
docker run --rm --network none --user 0:0 --entrypoint chown \
  --mount type=volume,source=diffpilot-local_server-data,target=/data,volume-nocopy \
  diffpilot-local-server:local "$(id -u):$(id -g)" /data
srl up -d --no-build --wait
srl ps
```

Register `http://localhost:3000/auth/github/callback` in the GitHub App. Open localhost (or tunnel it), confirm
preserved GitHub sign-in/settings, relink Codex in Settings after reading its logout-risk warning, then run a new
analysis and private Ask. SK1: re-enter a secret only if its setting was lost or deliberately reset; never export
secrets to a settings-transfer file. 0030 preserves stored application configuration, but not Codex linking.
After successful deployment and rollback retention, remove legacy containers referencing the old volumes and
run `deploy/cleanup-legacy-volumes.sh diffpilot-local --after-successful-deploy`. It refuses attached volumes,
only removes the four named legacy volumes, never reads them, and never revokes upstream credentials.

### Codex CLI upgrade (maintainers)

Update Codex package/checksum/notices in `deploy/worker/Dockerfile`, lean presets and app-server tests together.
The runner image contains no Codex binary. Rebuild server/worker together for checker ABI/format v3. Validate
with offline worker/launcher image tests, CS3-through-Squid, isolated fake-gateway networking, schema smoke and
both Playwright suites; auth fixtures are fake-only. Do not restore the removed real-login smoke harness.

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
