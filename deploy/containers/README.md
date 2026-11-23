# v1.3 container foundation (W3)

The approved runtime is [ADR-025](../../docs/decisions/ADR-025-container-runtime.md).
`compose.containers.yaml` is independent of the existing deployments. Its integration profile requires
W2's gateway and W4/W8's runner startup; it is not the deployment entry point yet.

1. Prepare the curated claudius context from the approved read-only checkout:

   ```sh
   deploy/containers/prepare-claudius.sh /path/to/claudius-at-460e5fd /data/tmp/v13w3-claudius
   ```

2. Build sequentially under the host's heavy-job lock. On a restricted host, set `DOCKER_CONFIG` to an empty
   writable directory under `/data/tmp`; do not copy the user's Docker credentials.

   ```sh
   flock /data/tmp/diffpilot-heavy.lock docker build -t v13w3-worker:local \
     --build-context claudius=/data/tmp/v13w3-claudius -f deploy/worker/Dockerfile .
   flock /data/tmp/diffpilot-heavy.lock docker build -t v13w3-fetch:local -f deploy/fetch/Dockerfile .
   flock /data/tmp/diffpilot-heavy.lock docker build -t v13w3-runner:local -f deploy/containers/runner.Dockerfile .
   ```

Pinned build bases (update deliberately together with the ABI smoke):

| Base | Digest |
|---|---|
| Rust / IM4 toolchain `1.100.0-x86_64-unknown-linux-gnu` | `sha256:7c7d4eb4d05b7781187a327809466cfddf29753f6defdc3d19a0e291951b7a37` |
| Debian bookworm-slim / IM2 and IM3 | `sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc` |

3. Supply `SR_WORKER_IMAGE` and `SR_FETCH_IMAGE` as repository references with `@sha256:` digests, and
   `SR_DOCKER_GID` as the Docker socket's group id. The launcher refuses tags and invalid resource limits.
   `SR_WORKER_MEMORY` is bytes, CPU count is integral, and `SR_WORKER_IDLE_STOP` is seconds.

4. Test dependency fetching without credentials:

   ```sh
   timeout 1200 flock /data/tmp/diffpilot-heavy.lock deploy/containers/test-cs3-squid.sh
   ```

   The test builds only `v13w3-` images and removes its containers, networks and volumes on exit. It exercises
   locked and lock-free fetches through Squid using JSON exported by the launcher's `Settings::template`
   (`cs3-template` example). Only the local image reference and the smoke assertion command are substituted;
   network, mounts, tmpfs and limits come from production code. It checks extracted sources, ignored checkout
   Cargo config, denied direct egress and cloud metadata. The fixture's build script fails if executed.

The launcher owns the Docker socket and has no network. Runner access uses the shared 0600 Unix socket;
workers receive only named data volumes. CS2 has a separate internal network without Squid. The gateway
must bind its model listener on both worker addresses and its admin listener only on the control address.
No worker publishes ports or joins the runner's control network.

`runner::launcher::Client` sends one newline-terminated JSON control request per connection. An attach
acknowledgement is followed by raw proxy stdout; stderr is kept out of the stream. Transport or control errors
close the connection. Detaching a relay refreshes last-use time and keeps the worker warm until its configured
idle stop (20 minutes by default). W4 must explicitly stop a worker when a relay is lost during a turn before
accepting another turn (FR2-2). Stop removes CS2's tmpfs state; remove also deletes CS1's work volume.
Failed CS3 fetches are always removed. Typed errors and fetch/no-lock outcomes require W4's response-contract
change; automatic fetch failure currently logs a warning and Ensure still acknowledges as before.

At runner startup call `Client::reconcile(retired_prs)` before accepting jobs, then reconcile the gateway's
local keys against all known worker identities, including missing workers. The launcher also stops existing
workers on its own startup/shutdown and cleans abandoned CS3 containers. List includes labeled work volumes
whose containers are missing as non-running workers; only an explicit Remove deletes their volumes. A later
Ensure recreates the container on the surviving volume. Work volumes are created with namespace,
shape, PR and user labels, including when Fetch precedes Ensure. Pre-release unlabeled volumes are reused
without deleting data (Docker cannot relabel an existing volume). W4 owns key revocation on stop,
relay loss and idle eviction; revoke the turn key before releasing the job attachment so autonomous idle/LRU
eviction cannot leave a live key. W8 derives retirement from PR/watch state. No upstream unlink is involved.

Every job calls Ensure before attaching. Image or template drift (including resource limits) recreates only
an unattached container and keeps its work volume. Per-job MemCan presence, URL and token do not affect the
template hash; credentials are absent from the container's environment and template labels.
Ensure regenerates job config using a fixed short-lived Docker exec, and app-server waits for the initial
runtime config. The token is passed only to that exec and written to `/run/dp/config.toml` on tmpfs;
`$CODEX_HOME/config.toml` links there. Codex 0.171 config edits resolve symlink targets, keeping trust/skill
writes on tmpfs. W4 supplies the fixed per-thread MCP configuration on every start/fork/resume (CCR2).

Before every job the reset empties HOME, including shell startup files and Git config. From CODEX_HOME it
removes instruction sources: `AGENTS*.md`, `CLAUDE*.md`,
`instructions.md`, skills, prompts, rules, hooks, plugins, config, `.codex`, `.claude`, `.agents` and `.config`.
It preserves thread transcripts and live app-server files, including `tmp/arg0-linux` helpers, SQLite databases
and WAL/SHM files, `installation_id` and `app-server-control/`. Removed symlinks are never followed. Checkout and
job files remain on `/work`; W4 resets inputs/outputs and frozen checkout state separately. The immutable system preset
and curated skills stay in the read-only image. W4 must repeat config hygiene before thread operations within
a job and must preserve this runtime symlink rather than writing a regular config on the volume.

CS3 fetch does not hold the lifecycle mutex. The service reserves each fetching identity against duplicate
fetch/ensure/attach/remove and idle/LRU eviction, while other workers remain available. The separate sweep
task skips a busy lifecycle lock, logs Docker failures and retries next interval; accept and signals keep running.

Ensure fetches dependency sources on cold/warm CS1 start. At first creation the checkout is still absent;
W4 must send `Fetch` after cloning it. Later per-job checkout updates and fixed kind configuration remain W4's
responsibility. Before every start/fork/resume, W4 rewrites the shape preset, keeps `/work` and `/work/repo`
untrusted, and uses CS1 cwd `/work` (CS2 `/tmp`). All CS2 setup/cleanup uses `fs/*`. The image does not implement
thread policies, schema handling, usage accounting or key lifecycle.
