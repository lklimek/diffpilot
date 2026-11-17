# e2e — full-stack Playwright

The thin slice through the real stack, no mocks in the browser: fake GitHub (`tools/fake-github`), server with
the embedded web UI, runner with the fake provider (fixture replay of `fixtures/pr-482`). The global setup
starts `deploy/dev/stack.sh` on free localhost ports with state under `/data/tmp`, and stops it afterwards.

Scenario (`tests/slice.spec.ts`): sign in as the fake user `alice` → paste
`https://github.com/acme/node-client/pull/482` → the analysis reaches `finished` (polled, 60 s bound) → the
review opens → PR context → flow 1 introduction → step 1 shows the fixture code. A second test checks that no
generated secret, OAuth token, session cookie or CSRF token appears in the component logs.

## Run

Requirements: Rust toolchain, Node 22, `git`, `curl`, `python3`, `sqlite3`.

```sh
(cd web && npm ci)                  # the launcher builds web/dist and the binaries
cd e2e
npm ci
npx playwright install chromium     # once per machine
npx playwright test                 # one run at a time; at most 2 workers
```

Prebuilt binaries skip the build (e.g. on a host where builds must go through a wrapper):
`SR_DEV_SERVER_BIN` (server, feature `embed-ui`, built after `npm run build` in `web/`), `SR_DEV_RUNNER_BIN`
(runner, feature `fake-provider`), `SR_DEV_FAKE_GITHUB_BIN`.

| Variable                  | Effect                                                                                                 |
| ------------------------- | ------------------------------------------------------------------------------------------------------ |
| `SR_E2E_STACK_ARGS`       | Extra launcher arguments, e.g. `--without runner` (the test then fails: the analysis never finishes).  |
| `SCREENSHOT_DIR`          | Saves sign-in, progress, PR context, flow introduction and step 1 at 1440 and 3440 px, light and dark. |
| `RUST_LOG`                | Log level of the components (default `info`).                                                          |
| `SR_E2E_START_TIMEOUT_MS` | Bound for the stack start, build included (default 15 min).                                            |

Component logs stay in the state directory the launcher prints (`<state>/logs/*.log`).

## The stack by hand

```sh
deploy/dev/stack.sh            # prints SR_DEV_URL=...; open it and sign in; Ctrl+C stops everything
deploy/dev/stack.sh --help     # every input and every variable it sets
deploy/dev/stack-check.sh      # launcher checks: git isolation, relative state dir, cleanup on start timeout
deploy/dev/real-codex-smoke.sh # one analysis of a public PR with the real Codex provider (see its header)
deploy/dev/smoke-publish-check.sh # smoke publication checks (fake credentials): marker names, leaks, failed scans
```
