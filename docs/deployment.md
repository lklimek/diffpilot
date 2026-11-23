# Deploying diffpilot

How to run diffpilot with Docker Compose or as a Portainer stack, from the published images or built from source.
The guide is independent of any particular host: every value in angle brackets (`<...>`) or in the shell variables
below is a placeholder for your environment. Day-to-day operation (backup schedule, logs, exit codes, credential
recovery) is in [runbook.md](runbook.md), which shows one concrete installation; its paths are examples.

Sources this guide follows: [`deploy/compose.yaml`](../deploy/compose.yaml) (the stack),
[`deploy/compose.secrets.yaml`](../deploy/compose.secrets.yaml) (secrets as files), [`.env.example`](../.env.example)
(the configuration template), [`deploy/check-production-config.sh`](../deploy/check-production-config.sh) (the
configuration check) and [`deploy/backup.sh`](../deploy/backup.sh) (online backup).

## 1. What runs

| Service | Image | Role | Listens |
|---|---|---|---|
| `server` | `lklimek/diffpilot` | Web UI and API, GitHub sign-in, job queue, SQLite database | `3000` (HTTP, published), `3001` (runner protocol, runner network only) |
| `runner` | `lklimek/diffpilot-runner` | Root supervisor, runner and the Codex agents (ADR-013) | nothing |
| `egress-proxy` | `lklimek/diffpilot-egress-proxy` | Squid explicit proxy: the runner's only way out | `3128` (runner network only) |

Three Docker networks with pinned subnets: `public` (server; GitHub API and the published port), `runner`
(internal and isolated: runner, the server's runner listener, the proxy), `egress` (proxy only; its way out). The
runner reaches nothing but the server's runner listener and the proxy. Every agent request leaves through the proxy,
which allows public destinations, logs host names and denies private, loopback and metadata ranges except the
configured MemCan endpoint.

Durable volumes: `<project>_server-data` (database, bare mirrors) and `<project>_runner-state` (Codex credential,
agent sessions). Disposable volumes `<project>_tmp-*` (label `diffpilot.volume=temporary`) hold scratch data and are
emptied at every start. `<project>` is the compose project name: `diffpilot` by default, the stack name in Portainer.

## 2. Prerequisites

- Linux host, `linux/amd64` (the published images are amd64 only).
- Docker Engine **28.0 or newer**: the runner network uses bridge gateway mode `isolated`; on older engines the
  runner's preflight refuses to start (exit 78). Check: `docker version -f '{{.Server.Version}}'`.
- Docker Compose **2.24.4 or newer** (`!reset` in `compose.secrets.yaml`), or Portainer (CE or BE) whose bundled
  Compose supports it.
- Memory: the stack's limits are runner 6 GB, server 2 GB, proxy 256 MB (variables in section 5). CPU: 4.5 cores
  by default.
- Disk: about 2 GB for the published images (about 15 GB more to build from source), plus the job work area: one
  full-history checkout per running job, without a size limit (a large repository takes a few GB). The runner takes
  no job while less than `SR_RUNNER_DISK_RESERVE_MIB` (default 2048) is free on Docker's data root.
- A domain name and a TLS-terminating reverse proxy in front of the server (section 7).
- A GitHub account that can create a GitHub App (section 4), and a ChatGPT account for the Codex sign-in (section 8).
- Outbound HTTPS from the host to GitHub, Docker Hub and the LLM provider (`chatgpt.com`, `openai.com`); with
  `SR_PR_HYGIENE_URL` set, also from the server to that PR Hygiene host; with `SR_THEPASTACLAW_STATUS_URL` set, to
  that host (`thepastaclaw.github.io`) (ADR-022). Notification channels (ntfy, webhook) and agent push targets are
  outbound destinations the users and their agents choose (ADR-023): the server reaches whatever `http`/`https`
  host they enter (e.g. `ntfy.sh`, a Home Assistant on the LAN), never loopback or link-local addresses;
  `SR_NOTIFY_DENY_PRIVATE=1` keeps it off private addresses too. Notifications connect directly (no proxy).
- Optional: a [MemCan](https://github.com/lklimek/memcan) instance (section 9).
- On the host for backups and checks: `bash`, `openssl`, `sqlite3` 3.27+ (`deploy/backup.sh`), `jq` (rendering
  check of `deploy/check-production-config.sh`).

Run the shell blocks below in a root shell (`sudo -i`): `.env`, the App key and the secret files are readable by
root only, and `docker compose` reads `.env`. They use three variables; set them in that shell first (placeholders,
any paths you choose):

```sh
DP_DIR=<deployment-directory>       # holds .env and, by default, the secrets directory and backups
DP_SECRETS="$DP_DIR/secrets"        # App key and, optionally, secret files
DP_REPO=<checkout-dir>              # the repository at the deployed tag
```

## 3. Images

### Published images

Release tags `vX.Y.Z` of the repository publish three images to Docker Hub (workflow
[`release-images.yml`](../.github/workflows/release-images.yml)):

| Image | Tags |
|---|---|
| `docker.io/lklimek/diffpilot` | `X.Y.Z`, `X.Y`, `X`, `latest` |
| `docker.io/lklimek/diffpilot-runner` | same |
| `docker.io/lklimek/diffpilot-egress-proxy` | same |

`X.Y.Z` never moves. `X.Y`, `X` and `latest` move to the newest release of their line. Deploy a pinned `X.Y.Z`,
and always run all three images at the **same** version: server and runner must match (they share the flow file
format), and the release workflow tests them together.

Published images carry OCI labels (`org.opencontainers.image.version`, `.revision` (commit), `.source`, `.licenses`
`FSL-1.1-ALv2`, `.created`):

```sh
docker image inspect -f '{{json .Config.Labels}}' docker.io/lklimek/diffpilot:<X.Y.Z>
```

Licences: diffpilot is [FSL-1.1-ALv2](../LICENSE.md). Every image contains the licence text, the list of its Ubuntu
packages with their source packages (`packages.txt`) and the notices of the bundled third-party software in
`/usr/share/doc/diffpilot/` (layout, sources and known gaps: [`deploy/notices/README.md`](../deploy/notices/README.md));
Ubuntu packages keep their copyright files in `/usr/share/doc/<package>/copyright`. The source code of the Ubuntu
packages, GPL-licensed ones (Squid, git, bash) included, is published by Ubuntu for exactly the versions listed in
`packages.txt`.

The compose file takes the image names from `SR_SERVER_IMAGE`, `SR_RUNNER_IMAGE` and `SR_PROXY_IMAGE`; set them to
the published names (section 5).

### Building from source

Check out the release tag and build on the host; the compose file's `build:` sections are used when the image
variables keep their `*:local` defaults. Builds are heavy (Rust release builds, capped at two compiler jobs): build
one image at a time.

```sh
git clone --branch v<X.Y.Z> https://github.com/lklimek/diffpilot.git <checkout-dir>
cd <checkout-dir>
docker compose --env-file <env-file> -f deploy/compose.yaml build egress-proxy
docker compose --env-file <env-file> -f deploy/compose.yaml build runner
docker compose --env-file <env-file> -f deploy/compose.yaml build server
```

A deployment from published images needs the repository only for `.env.example`, the compose files and the
scripts (`deploy/check-production-config.sh`, `deploy/backup.sh`); use the same tag as the images.

## 4. GitHub App

Create the App under the account or organisation that owns the deployment (GitHub → Settings → Developer settings
→ GitHub Apps → New GitHub App):

| Field | Value |
|---|---|
| Homepage URL | `<public-url>` (the value of `SR_PUBLIC_URL`) |
| Callback URL | exactly `<public-url>/auth/github/callback` (no wildcard; one callback per deployment) |
| Expire user authorization tokens | on |
| Request user authorization (OAuth) during installation | off |
| Webhook | off (diffpilot uses no webhook) |
| Repository permissions | Pull requests: read and write; Contents, Checks, Issues, Metadata: read-only |
| Account and organisation permissions | none |
| Where can this App be installed | as you need (only the owner's account, or any account) |

Then:

1. Note the **App ID** (`SR_GITHUB_APP_ID`) and the **Client ID** (`SR_GITHUB_CLIENT_ID`).
2. Generate a **client secret** (`SR_GITHUB_CLIENT_SECRET`).
3. Generate a **private key** (`.pem`). It stays on the host: the server reads it from a read-only bind mount
   (`SR_GITHUB_APP_PRIVATE_KEY_PATH`), never from an image or volume. Make it readable by the server's uid 10010
   only:

   ```sh
   install -d -m 0755 "$DP_SECRETS"
   install -o 10010 -g 10010 -m 0400 <downloaded-key>.pem "$DP_SECRETS/github-app.pem"
   ```

4. Install the App on the repositories to review. v1 analyses public repositories; without an installation on a
   repository, pull requests can be analysed but comments must be written on GitHub.

## 5. Configuration

Start from the template and generate the two server secrets (the file holds secrets: owner root, mode 0600):

```sh
install -d -m 0755 "$DP_DIR"
install -m 0600 "$DP_REPO/.env.example" "$DP_DIR/.env"
sed -i \
  -e "s|^SR_SECRET_KEY=.*|SR_SECRET_KEY=$(openssl rand -base64 32)|" \
  -e "s|^SR_RUNNER_SECRET=.*|SR_RUNNER_SECRET=$(openssl rand -hex 32)|" \
  "$DP_DIR/.env"
"${EDITOR:-vi}" "$DP_DIR/.env"
```

`SR_SECRET_KEY` encrypts stored GitHub tokens and cannot be rotated: keep a copy with the backups. Losing it means
every user signs in again.

### Variables

Required means compose or the service refuses to start without it. "Secret" values can come from files instead
(below).

**Server and GitHub**

| Variable | Required | Default | Meaning |
|---|---|---|---|
| `SR_PUBLIC_URL` | yes | | Public base URL, `https://<host-name>`. The OAuth callback is derived from it; `https://` makes cookies `Secure`. |
| `SR_ALLOWED_LOGINS` | yes | | GitHub logins allowed to sign in, comma-separated, case-insensitive (section 6, "Sign-in allow-list"). |
| `SR_SECRET_KEY` | yes, secret | | 32 random bytes, base64 (`openssl rand -base64 32`). |
| `SR_RUNNER_SECRET` | yes, secret | | Shared by server and runner, at least 32 characters (`openssl rand -hex 32`). |
| `SR_GITHUB_APP_ID` | yes | | App ID. |
| `SR_GITHUB_CLIENT_ID` | yes | | App client ID. |
| `SR_GITHUB_CLIENT_SECRET` | yes, secret | | App client secret. |
| `SR_GITHUB_APP_PRIVATE_KEY_PATH` | yes | | Host path of the App key (`.pem`, readable by uid 10010); mounted read-only. |
| `SR_GITHUB_API_URL` | no | `https://api.github.com` | Change only for GitHub Enterprise Server. |
| `SR_GITHUB_OAUTH_URL` | no | `https://github.com` | Same. |
| `SR_GITHUB_CLONE_URL` | no | `https://github.com` | Same. |
| `SR_PUBLISH_ADDR` | no | `127.0.0.1` | Host address the HTTP port is published on (the reverse proxy's side). |
| `SR_HTTP_PORT` | no | `3000` | Host port of the server's HTTP listener. |
| `SR_TRUSTED_PROXIES` | no | empty | Comma-separated addresses or CIDRs whose `X-Forwarded-For` and `X-Forwarded-Proto` are trusted (section 7). |
| `SR_MEMCAN_URL` | yes | | MemCan MCP endpoint, full URL (section 9). |
| `SR_MEMCAN_TOKEN` | yes, secret | | MemCan token (section 9). |
| `SR_MEMCAN_UI_URL` | no | empty | MemCan web UI base including `/ui`; the dashboard links TODOs to `<base>/tasks/<uuid>`. |
| `SR_PR_HYGIENE_URL` | no | empty | PR Hygiene service base URL (`http`/`https`), e.g. `https://pr-hygiene.services.dash.org`. Set: the dashboard plug-in reads its public API (no credential) at most once a minute while someone uses the dashboard or any user watches a pull request (ADR-022). Empty: off. |
| `SR_THEPASTACLAW_STATUS_URL` | no | empty | URL of thepastaclaw's review-system status file (`http`/`https`), e.g. `https://thepastaclaw.github.io/review-system/data/status.json`. Set: the dashboard plug-in reads it (no credential) at most once a minute while someone uses the dashboard or any user watches a pull request and notes which pull requests thepastaclaw reviews or has queued (ADR-022). Empty: off. |
| `SR_NOTIFY_DENY_PRIVATE` | no | `0` | `1`: notification channels and agent push targets may not reach private (RFC 1918), unique-local or shared (`100.64.0.0/10`) addresses either. `0`: they may (Home Assistant, a self-hosted ntfy on the LAN). Loopback, link-local (`169.254.0.0/16`, `fe80::/10`), unspecified, multicast, broadcast, `198.18.0.0/15`, `240.0.0.0/4` and IPv6 forms embedding such an address (IPv4-mapped, NAT64, 6to4) and Teredo (`2001::/32`) are refused with either value. An address in the URL is refused when saved; a host name is resolved by the server and only its allowed addresses are used; notifications never use a proxy from the environment. Other values stop the server at start (exit 1). |
| `RUST_LOG` | no | `info` | Log level of server and runner (`debug`, or per module, e.g. `info,runner=debug`). |

**Runner**

| Variable | Required | Default | Meaning |
|---|---|---|---|
| `SR_RUNNER_ID` | no | `runner` | Runner name shown in logs. |
| `SR_RUNNER_CLAUDIUS_URL` | no | `https://github.com/lklimek/claudius.git` | Repository the agents' skills come from. |
| `SR_RUNNER_CLAUDIUS_REF` | no | `main` | Branch, tag (`refs/tags/<tag>`) or commit SHA of the skills; a SHA or tag pins them (recommended; the check warns on a branch). |
| `SR_RUNNER_CLAUDIUS_REFRESH_SECS` | no | `21600` | Refresh interval of the skills checkout. |
| `SR_RUNNER_SKILLS` | no | `severity` | Skills a job may get (analyses and Ask read `severity`; comment assessments read none). A job asking for a skill not listed fails. A list that still names `check-pr-comments` works; that skill is no longer given to any job. |
| `SR_RUNNER_SESSION_RETENTION_DAYS` | no | `14` | Days an agent session is kept after its last job. |
| `SR_RUNNER_DISK_RESERVE_MIB` | no | `2048` | Free space on Docker's data root below which the runner takes no job (and is unhealthy). |
| `SR_SUPERVISOR_SWEEP_DEADLINE_MS` | no | `5000` | Deadline of the job-end process sweep; a miss ends the container (exit 70). |

**Images, limits and networks**

| Variable | Default | Meaning |
|---|---|---|
| `SR_SERVER_IMAGE` | `diffpilot-server:local` | Published: `docker.io/lklimek/diffpilot:<X.Y.Z>`. |
| `SR_RUNNER_IMAGE` | `diffpilot-runner:local` | Published: `docker.io/lklimek/diffpilot-runner:<X.Y.Z>`. |
| `SR_PROXY_IMAGE` | `diffpilot-egress-proxy:local` | Published: `docker.io/lklimek/diffpilot-egress-proxy:<X.Y.Z>`. |
| `SR_SERVER_MEM_LIMIT`, `SR_SERVER_CPUS` | `2g`, `2` | Server limits. |
| `SR_RUNNER_MEM_LIMIT`, `SR_RUNNER_CPUS`, `SR_RUNNER_PIDS_LIMIT` | `6g`, `2`, `1024` | Runner limits (agents included). |
| `SR_PUBLIC_SUBNET`, `SR_SERVER_PUBLIC_ADDR` | `172.29.73.0/24`, `172.29.73.2` | `public` network and the server's address on it. |
| `SR_RUNNER_SUBNET`, `SR_SERVER_RUNNER_ADDR`, `SR_PROXY_RUNNER_ADDR`, `SR_RUNNER_ADDR` | `172.29.71.0/24`, `.2`, `.3`, `.4` | `runner` network and the addresses on it. |
| `SR_EGRESS_SUBNET`, `SR_PROXY_EGRESS_ADDR` | `172.29.72.0/24`, `172.29.72.2` | `egress` network and the proxy's address (MemCan sees agent requests from it). |
| `SR_SECRETS_DIR` | `/srv/diffpilot/secrets` | Directory of the secret files (with `compose.secrets.yaml` only); set it explicitly. |

Change the subnets only when they clash with existing networks; keep each address inside its subnet.

The containers fix the rest themselves (data paths, listen ports, the runner's state layout); do not set
`SR_DATA_DIR`, `SR_LISTEN_ADDR`, `SR_RUNNER_LISTEN_ADDR`, `SR_RUNNER_STATE_DIR` or `SR_RUNNER_WORK_DIR`.

### Secrets as files (optional)

With `deploy/compose.secrets.yaml` on top of `compose.yaml`, the four secrets (`SR_SECRET_KEY`, `SR_RUNNER_SECRET`,
`SR_GITHUB_CLIENT_SECRET`, `SR_MEMCAN_TOKEN`) come from read-only files in `SR_SECRETS_DIR`, so they do not show in
`docker inspect` or Portainer. Leave the four variables empty in `.env` and set `SR_SECRETS_DIR`. The runner
secret is read by the server (uid 10010) and the runner (uid 10001), never by the agents (uid 10002; the runner
refuses to start otherwise):

```sh
cd "$DP_SECRETS"
sh -c 'umask 077; openssl rand -base64 32 > sr-secret-key; openssl rand -hex 32 > sr-runner-secret'
sh -c 'umask 077; cat > sr-github-client-secret'   # paste the value, Enter, Ctrl-D
sh -c 'umask 077; cat > sr-memcan-token'           # paste the value, Enter, Ctrl-D
chown 10010:10010 sr-secret-key sr-github-client-secret sr-memcan-token
chmod 0400 sr-secret-key sr-github-client-secret sr-memcan-token
chown 10001:10010 sr-runner-secret && chmod 0440 sr-runner-secret
sed -i -E 's/^(SR_SECRET_KEY|SR_RUNNER_SECRET|SR_GITHUB_CLIENT_SECRET|SR_MEMCAN_TOKEN)=.*/\1=/' "$DP_DIR/.env"
echo "SR_SECRETS_DIR=$DP_SECRETS" | tee -a "$DP_DIR/.env" > /dev/null
```

### Checking the configuration

Run the check from a clean shell (variables of the invoking shell override the file in compose); it never prints
values. It refuses values left from `.env.example`, empty secrets, a runner secret under 32 characters, a public URL
that is not `https://`, `$` interpolation and development switches; an unpinned skills ref and an empty
`SR_TRUSTED_PROXIES` are warnings.

```sh
"$DP_REPO/deploy/check-production-config.sh" --env-file "$DP_DIR/.env"                  # secrets in .env
"$DP_REPO/deploy/check-production-config.sh" --env-file "$DP_DIR/.env" --secret-files   # secrets as files
```

## 6. Deploying

### Docker Compose

```sh
dp() { docker compose --env-file "$DP_DIR/.env" -f "$DP_REPO/deploy/compose.yaml" "$@"; }
# with secrets as files instead:
# dp() { docker compose --env-file "$DP_DIR/.env" -f "$DP_REPO/deploy/compose.yaml" \
#   -f "$DP_REPO/deploy/compose.secrets.yaml" "$@"; }
dp pull                  # published images
dp up -d --no-build      # --no-build: a failed pull never falls back to a build on the host
dp ps                    # all three healthy within about a minute
```

Built from source: build as in section 3, then `dp up -d`.

### Portainer

Name the stack `diffpilot`: the stack name is the compose project name, so volumes become `diffpilot_server-data`
and so on, as in the runbook. The App key, and the secret files if used, must exist on the Docker host at the paths
the variables name (bind mounts are host paths, not Portainer paths).

**From the repository (recommended).** Stacks → Add stack → Repository:

- Repository URL `https://github.com/lklimek/diffpilot`, reference `refs/tags/v<X.Y.Z>` (the release of the images).
- Compose path `deploy/compose.yaml`; for secrets as files add the additional path `deploy/compose.secrets.yaml`.
- Environment variables: "Load variables from .env file" with a filled-in copy of `.env.example`, or enter them
  one by one; at least the required ones of section 5 and the three image variables.
- Optional "Re-pull image" on updates. Keep automatic updates off, or pointed at a tag: upgrades need a backup first
  (section 11).
- Check that the image tags exist on Docker Hub first: in this mode the checkout is a build context, so a failed
  pull (a mistyped tag) makes compose build the images on the host instead, which is slow and needs about 15 GB.

**Web editor or upload.** Paste or upload `deploy/compose.yaml` of the release tag unchanged and set the variables
as above. The `build:` sections are never used while the image variables name published images; if a pull fails,
the deployment fails instead of building. `compose.secrets.yaml` cannot be added in this mode, so the secrets are
stack variables: visible to Portainer users with access to the stack and in `docker inspect`.

Portainer stores stack variables in its own database: keep a copy of `SR_SECRET_KEY` with your backups. For the
commands below and in sections 8 to 12, use the Docker CLI on the host (`docker exec`, `docker run` against the
volumes) or the container console in Portainer (Containers → `<container>` → Console, with the user named there).

### Sign-in allow-list

Only the GitHub logins in `SR_ALLOWED_LOGINS` can sign in, for example `SR_ALLOWED_LOGINS=octocat,hubot`. The server
applies the list at every start, before it serves a request: it adds missing logins and removes the others. A
login is bound to its GitHub account at its first sign-in; removing it ends that account's sessions, deletes its
stored GitHub tokens and revokes its personal access tokens, so listing it again later needs a new sign-in and new
access tokens. The start log line `sign-in allowlist set from SR_ALLOWED_LOGINS` names the logins added and removed.
Changing the list takes a restart of `server` (`dp up -d server`).

The server refuses to start without the variable, and with an entry that is not a GitHub login (letters, digits,
`-` or `_`, at most 39) or a login listed twice.

**Databases created before `SR_ALLOWED_LOGINS`** were seeded with the maintainers' logins `lklimek` and
`Claudius-Maginificent`. An upgrade drops them while nobody has signed in through them; afterwards the variable
decides, so set it to every login that should keep access before upgrading. The manual allow-list edits of earlier
versions (`sqlite3 … tenant_allowed_logins`) are no longer needed: the next start replaces them with the variable.

## 7. Reverse proxy and TLS

Route to the server only once `SR_ALLOWED_LOGINS` holds your logins (section 6, "Sign-in allow-list"). The server
speaks plain HTTP on `SR_PUBLISH_ADDR:SR_HTTP_PORT` (default `127.0.0.1:3000`); TLS terminates at a reverse proxy
you run in front of it, for the host name of `SR_PUBLIC_URL`. Requirements:

- Forward everything under `/` to the server; the server routes `/`, `/api/`, `/auth/` and `/healthz` itself.
- Append the client address to `X-Forwarded-For` (nginx: `proxy_set_header X-Forwarded-For
  $proxy_add_x_forwarded_for;`) and set `X-Forwarded-Proto` (nginx: `proxy_set_header X-Forwarded-Proto $scheme;`).
  Failed access-token attempts are limited per client address (30 a minute); without the header all clients share
  the proxy's address and one limit.
- Set `SR_TRUSTED_PROXIES` to the proxy's address **as the container sees it**: with the proxy on the host and
  Docker's userland proxy (the default), requests come from the `public` network's gateway (`172.29.73.1` with the
  default subnet); with the proxy in a container on the `public` network, its container address.
- Server-sent events: the UI keeps long-lived event streams open (a heartbeat every 15 s). Disable response
  buffering (nginx: `proxy_buffering off;`) and keep the read timeout above 15 s.
- Publish the HTTP port only where the reverse proxy reaches it (loopback when it runs on the host). The runner
  listener (`3001`) is never published.

## 8. First start

1. **Start** (section 6, with `SR_ALLOWED_LOGINS` set) and wait until `server`, `runner` and `egress-proxy` are
   `healthy`.

2. **Codex sign-in** (once per `runner-state` volume). The runner keeps one Codex credential in its state volume;
   sign in inside the running runner container as the runner uid (10001). Codex reaches the Internet through the
   egress proxy. Never mount or copy a host's `~/.codex`, and never copy a credential another Codex installation
   still uses (refresh tokens are single-use).

   ```sh
   docker exec -u 10001 <project>-runner-1 install -d -m 0700 /var/lib/diffpilot-runner/credentials/default
   docker exec -it -u 10001 -e CODEX_HOME=/var/lib/diffpilot-runner/credentials/default <project>-runner-1 \
     codex login --device-auth
   docker exec -u 10001 <project>-runner-1 test -s /var/lib/diffpilot-runner/credentials/default/auth.json \
     && echo "credential present"
   ```

   The device login prints a URL and a code: open the URL, enter the code, approve. In Portainer: runner container →
   Console, user `10001`, command `/bin/bash`, then the same commands with `CODEX_HOME=...` prefixed.

3. **First sign-in.** Open `<public-url>`, choose "Sign in with GitHub", approve the App. Then Settings: model and
   effort, dashboard and the other options ([settings.md](settings.md); in the app: Settings → "Settings help").

4. **Known bots.** Every user's dashboard starts with the project's review bots `thepastaclaw`,
   `Claudius-Maginificent` and `DCG-Claude` as known bots (their review requests and comments do not count as a
   person waiting for you). Each user changes or clears the list in Settings → Dashboard → "Known bots"; GitHub Apps
   (`...[bot]`) are bots without being listed.

## 9. MemCan (optional service, required variables)

diffpilot uses MemCan for agent memories, feedback, the project profile and dashboard TODOs. The server refuses to
start without `SR_MEMCAN_URL` and `SR_MEMCAN_TOKEN`, but MemCan itself may be absent: everything else works, the
agents run without memories and the dashboard reports MemCan as unavailable.

- **With MemCan:** `SR_MEMCAN_URL` is the full MCP endpoint (`http(s)://<memcan-host>:<port>/mcp`; nothing is
  appended), `SR_MEMCAN_TOKEN` its API token. The server connects directly; the agents connect through the egress
  proxy, which allows exactly that host name, port and the address the name resolved to at the proxy's start. On
  the MemCan side, allow the host name (MemCan's allowed hosts) and both source addresses: the server's and the
  proxy's. If MemCan runs on the same Docker host, these are `SR_SERVER_PUBLIC_ADDR` and `SR_PROXY_EGRESS_ADDR`;
  on another host, the Docker host's own address (Docker masquerades). If MemCan's address changes, restart the
  proxy.
- **Without MemCan:** an endpoint that refuses connections at once, with an IP literal so the proxy needs no DNS,
  and any token, e.g. `SR_MEMCAN_URL=http://127.0.0.1:9/mcp`.

Checking both paths, and what each answer means: [runbook.md](runbook.md), "First deploy", step 5.

## 10. Health checks

Each image has a healthcheck; `docker compose ps` (or Portainer's container list) shows `healthy`:

| Service | Healthy when |
|---|---|
| `server` | `GET /healthz` on its HTTP listener answers `204`. |
| `runner` | the runner process runs (its preflight passed) and the disk-reserve marker is absent. |
| `egress-proxy` | Squid answers an HTTP request on `3128`. |

The runner starts only after the server and the proxy are healthy. For external monitoring, probe
`<public-url>/healthz` (expects `204`, no body, no authentication).

## 11. Backup and upgrade

**Backup.** The durable state is the two volumes `server-data` and `runner-state`, plus `SR_SECRET_KEY` (in `.env`
or the secret files, or in Portainer) and the App key. Online backup while running (root; `sqlite3` 3.27+ on the
host):

```sh
SR_COMPOSE_PROJECT=<project> SR_BACKUP_DIR="$DP_DIR/backup" SR_ENV_FILE="$DP_DIR/.env" \
  SR_SECRETS_DIR="$DP_SECRETS" "$DP_REPO/deploy/backup.sh"
```

It writes a consistent database snapshot, the Codex credential and agent sessions, `.env` and the secrets directory
(when present) to a dated directory, keeps the newest 14 (`SR_BACKUP_KEEP`) and prints sizes only. The backup holds
credentials: encrypt it before it leaves the host. Scheduling, restore (`--restore`), full backups with the stack
stopped and off-site copies: [runbook.md](runbook.md), "Backup and restore".

**Upgrade.** Database migrations are numbered, append-only and applied automatically when the server starts, each
in its own transaction; a failed one leaves the database at the previous version. An older server refuses a database
that a newer one has migrated, so a rollback always means restoring the backup taken before the upgrade.

1. Read the version-specific upgrade sections of [runbook.md](runbook.md) (such as "Upgrade to v1.1") for every
   version in between; upgrade all three images to the same new tag.
2. Back up (above) with the services stopped, or take a full backup ([runbook.md](runbook.md), "Full backup").
3. Point the three image variables at the new `X.Y.Z` (and, for files from the repository, the checkout or the
   Portainer reference at `v<X.Y.Z>`), then:

   ```sh
   dp pull && dp up -d --no-build
   dp ps
   dp logs --since 10m server | grep -E 'server listening|schema migration failed'
   ```

   In Portainer: update the variables (and the reference), then "Update the stack" with "Re-pull image".
4. Verify: all three `healthy`, sign-in works, an existing analysis opens. `the schema migration failed` in the
   server log (exit 1, restart loop) means the image does not match the database: roll back.

**Rollback:** stop the stack, restore the step-2 backup, set the image variables back to the previous tag, start.
Never delete the database to get past a migration error.

## 12. Troubleshooting

| Symptom | Cause and action |
|---|---|
| `required variable ... is missing a value` from compose | A required variable of section 5 is unset in `.env` or the Portainer stack. |
| Runner exits 78 | The runner network is not isolated (Docker Engine older than 28.0, or the network options changed: `docker compose down` and `up -d` re-create networks), or the runner secret file is readable by uid 10002 (section 5). |
| Runner exits 70 or 71 | Job-end sweep missed its deadline (restarts clean), or the supervisor could not set up (compose `cap_add`, `init`, `ipc` changed). [runbook.md](runbook.md), "Exit codes and restarts". |
| Egress proxy exits 64 or 65 | `SR_MEMCAN_URL` is not `http(s)://host[:port][/path]` (64), or its host name does not resolve (65). |
| Server exits 1 | The last log line names the variable (configuration), or `the schema migration failed` (section 11). |
| Runner `unhealthy`, jobs stay queued | Less free disk than `SR_RUNNER_DISK_RESERVE_MIB`; free space, e.g. remove the `tmp-*` volumes with the stack stopped ([runbook.md](runbook.md), "Temporary volumes"). |
| Sign-in shows "This GitHub login is not allowed." (`login_error=login_not_allowed` in the URL) | The login is not in `SR_ALLOWED_LOGINS` (section 6, "Sign-in allow-list"); after changing it restart `server`. |
| GitHub says the redirect URI is not associated | The App's callback URL differs from `<SR_PUBLIC_URL>/auth/github/callback`. |
| Settings shows "Sign-in to the LLM provider is needed" | The Codex credential is missing or was rejected: sign in again (section 8, step 2). |
| Analyses stop with live updates missing | The reverse proxy buffers or cuts event streams (section 7). |
| Agents cannot reach MemCan | MemCan's host or address allow-list (section 9); proxy started before a MemCan address change: restart it. |

Logs: `dp logs -f --since 1h server runner`; proxy access log (host names only): `dp logs egress-proxy`. More
detail: `RUST_LOG=debug`, then `dp up -d server runner`.

## 13. Publishing images (maintainers)

The workflow [`release-images.yml`](../.github/workflows/release-images.yml) runs on a pushed tag `vX.Y.Z` of this
repository (never on forks or pull requests), in two jobs:

1. `build`, without credentials: builds the three images (linux/amd64) with their licence notices and OCI labels,
   checks the notices, runs the runner container suite, the egress proxy test and the stack test against exactly
   these images, and hands them to the next job as a workflow artifact (kept seven days).
2. `publish`, in the environment `docker-hub`, one at a time across releases: loads the tested images, computes the
   tags of section 3 from the release tags present at that moment (it stops when it cannot read them), logs in,
   refuses to continue when `X.Y.Z` of any of the three images already exists (a release version is published once),
   pushes tag by tag across the three images (`X.Y.Z` first, then `X.Y`, `X`, `latest`) and logs out.

A manual run (Actions → Release images → Run workflow, with a version) is a dry run: `build` only, printing the
tags a release would push, without credentials and without pushing.

Recovery: a failed or cancelled publish that pushed no `X.Y.Z` is re-run with "Re-run failed jobs" while the
artifact exists (seven days), else with "Re-run all jobs" (builds and tests again). After a partial push (some
`X.Y.Z` exist), release the next patch version. GitHub keeps at most one waiting publish: when a third release is
tagged while one publish runs and another waits, the waiting one is cancelled and must be re-run. Push release
tags one at a time, each after the previous publish has finished.

Setup on GitHub, once:

1. Docker Hub: create the repositories `lklimek/diffpilot`, `lklimek/diffpilot-runner` and
   `lklimek/diffpilot-egress-proxy` (public; the publish job stops when one is missing), and a personal access token
   with **Read & Write** scope.
2. GitHub → Settings → Environments → New environment `docker-hub`; under "Deployment branches and tags" allow only
   tags matching `v*`; optionally add yourself as a required reviewer. Add the environment secrets:
   - `DOCKERHUB_USERNAME`: the Docker Hub user that owns the token;
   - `DOCKERHUB_TOKEN`: the access token.

   (Repository secrets with the same names also work, without the tag restriction.)

Releasing: tag the commit and push the tag (`git tag v<X.Y.Z> && git push origin v<X.Y.Z>`). Never move or re-push
a published release tag: deployments pin `X.Y.Z` and expect it never to change.
