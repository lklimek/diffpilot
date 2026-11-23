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

3. Supply `SR_WORKER_IMAGE` and `SR_FETCH_IMAGE` as repository references with `@sha256:` digests, and
   `SR_DOCKER_GID` as the Docker socket's group id. The launcher refuses tags and invalid resource limits.
   `SR_WORKER_MEMORY` is bytes, CPU count is integral, and `SR_WORKER_IDLE_STOP` is seconds.

4. Test dependency fetching without credentials:

   ```sh
   timeout 1200 flock /data/tmp/diffpilot-heavy.lock deploy/containers/test-cs3-squid.sh
   ```

   The test builds only `v13w3-` images and removes its containers, networks and volumes on exit. It exercises
   locked and lock-free fetches through Squid, extracted sources, ignored checkout Cargo config, denied direct
   egress and denied cloud metadata. The fixture's build script fails if executed.

The launcher owns the Docker socket and has no network. Runner access uses the shared 0600 Unix socket;
workers receive only named data volumes. CS2 has a separate internal network without Squid. The gateway
must bind its model listener on both worker addresses and its admin listener only on the control address.
No worker publishes ports or joins the runner's control network.

`runner::launcher::Client` sends one newline-terminated JSON control request per connection. An attach
acknowledgement is followed by raw proxy stdout; stderr is kept out of the stream. Transport or control errors
close the connection. A relay ending stops its worker before another attachment can restart it. Stop removes
CS2's tmpfs state; remove also deletes CS1's work volume. Failed CS3 fetches are removed and reported so the
caller can record unavailable dependency sources.

At runner startup call `Client::reconcile(retired_prs)` before accepting jobs, then reconcile the gateway's
local keys against all known worker identities, including missing workers. The launcher also stops existing
workers on its own startup/shutdown and cleans abandoned CS3 containers. W4 owns key revocation on stop,
relay loss and idle eviction; revoke the turn key before releasing the job attachment so autonomous idle/LRU
eviction cannot leave a live key. W8 derives retirement from PR/watch state. No upstream unlink is involved.

Ensure fetches dependency sources on cold/warm CS1 start. At first creation the checkout is still absent;
W4 must send `Fetch` after cloning it. Later per-job checkout updates and fixed kind configuration remain W4's
responsibility. Before every start/fork/resume, W4 rewrites the shape preset, keeps `/work` and `/work/repo`
untrusted, and uses CS1 cwd `/work` (CS2 `/tmp`). All CS2 setup/cleanup uses `fs/*`. The image does not implement
thread policies, schema handling, usage accounting or key lifecycle.
