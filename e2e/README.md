# e2e — full-stack Playwright

The v1 user journeys through the real stack, no mocks in the browser: server with the embedded web UI, runner
with the fake provider (fixture replay of `fixtures/pr-482`, scriptable), fake GitHub (`tools/fake-github`) and
fake MemCan (`tools/fake-memcan`). Every reached state is also checked for the wording rules of brief §1 (no
"no risk", "verified", "noise", "confidence", percentages …) and by axe (serious or critical = failure).

| Spec                      | Journey                                                                                                 |
| ------------------------- | ------------------------------------------------------------------------------------------------------- |
| `slice.spec.ts`           | sign in, analyse PR 482, flow 1 step 1; no secret or token in any component log                         |
| `analysis-cancel.spec.ts` | planning, cancel → "analyze again"; failure before any flow → Retry → `k of n` → finished               |
| `analysis-pause.spec.ts`  | usage limit → "Paused until HH:MM" → resumed; failed flow named → Retry → finished                      |
| `tour.spec.ts`            | PR context facts, flow order, quiet rows, no LLM questions (hard case 3), classic diff, resume, columns |
| `comments.spec.ts`        | pending review: create, second comment, edit, delete, outside the diff, missed by the LLM (hard case 2) |
| `head-moved.spec.ts`      | `pr_head_changed`, "The PR has moved" → Re-analyze without ticks, Ask on the superseded analysis        |
| `feedback.spec.ts`        | wrong + overrated → MemCan calibration example (hard case 1), pending → written, removal, rejected      |
| `ask.spec.ts`             | answered and pinned, usage-limit wait with its time, failed answer as a sentence, markup as text        |
| `settings.spec.ts`        | signed-out screens, LLM status after a failed and a working call, language                              |
| `security.spec.ts`        | CSP header; an inline script does not run; LLM analysis text with markup renders as text                |

## Run

Requirements: Rust toolchain, Node 22, `git`, `curl`, `openssl`, `python3`, `sqlite3`.

```sh
(cd web && npm ci)                  # the global setup builds web/dist and the binaries
cd e2e
npm ci
npx playwright install chromium     # once per machine
npx playwright test                 # one run at a time; at most 2 workers
npx playwright test tests/ask.spec.ts
```

The global setup builds once (`npm run build` in `web/`, then `cargo build` of server with `embed-ui`, runner with
`fake-provider`, `fake-github`, `fake-memcan`). Prebuilt binaries skip the build, e.g. on a host where builds go
through a wrapper: set all four of `SR_DEV_SERVER_BIN`, `SR_DEV_RUNNER_BIN`, `SR_DEV_FAKE_GITHUB_BIN`,
`SR_DEV_FAKE_MEMCAN_BIN`.

| Variable                  | Effect                                                                          |
| ------------------------- | ------------------------------------------------------------------------------- |
| `SR_E2E_OUTPUT_DIR`       | Traces and screenshots of failures (default `test-results`).                    |
| `SR_E2E_STACK_ARGS`       | Extra launcher arguments, e.g. `--without runner` (analyses then never finish). |
| `SR_E2E_START_TIMEOUT_MS` | Bound for one stack start (default 2 min).                                      |
| `SCREENSHOT_DIR`          | `slice.spec.ts` saves its screens at 1440 and 3440 px, light and dark.          |
| `RUST_LOG`                | Log level of the components (default `info`).                                   |

## How the stack is started

Each spec file gets its own stack, started with its first test and stopped when the worker moves on
(`support/fixtures.ts`, `support/stack.ts`): `deploy/dev/stack.sh` on free localhost ports, state and logs under
`/data/tmp/smart-review-dev.*` (kept; the launcher prints the path). Tests of one file share that stack and run in
order. The server allows one live analysis per PR head and language, so a file that needs a fresh analysis
(cancel, failure, pause) lives in a file of its own.

What a test can reach of a local stack (`Stack` in `support/stack.ts`):

- `stack.script(rules)` writes the fake provider's script (`SR_RUNNER_FAKE_SCRIPT`, re-read before every agent
  call): per job kind, flow and prompt substring, `replay` slowly, `usage_limit` with a reset time, `fail`, or
  `answer` with a given output (Ask). Format: `crates/runner/src/provider/fake.rs`.
- `stack.memcan` with `memcan(page, stack)` (`support/app.ts`): fake MemCan's memories, deletions and calls
  (`GET /_fake/state`) and its mode (`normal`, `reject`, `unavailable`, `hang`, `delay`).
- `stack.moveHead()` pushes a commit onto the PR head in the clone base and moves fake GitHub's head
  (`POST /_fake/head`).

## Against already-running services

Point the suite at a stack started elsewhere (e.g. compose with the fake provider) instead of starting its own:

| Variable                 | Meaning                                                                |
| ------------------------ | ---------------------------------------------------------------------- |
| `SR_E2E_BASE_URL`        | Public URL of the server; nothing is built or started; one worker.     |
| `SR_E2E_FAKE_MEMCAN_URL` | Base URL of the fake MemCan the server uses (for the feedback checks). |
| `SR_E2E_MEMCAN_TOKEN`    | Its bearer token (`SR_MEMCAN_TOKEN` of the server).                    |

The stack must serve fake GitHub's fixture PR and allow the login `alice`. Tests that need the runner's script,
the clone base or the component logs skip themselves; the spec files share the one stack, so the first analysis
of PR 482 is reused by the later files. Not wired into CI yet: the compose-based run waits for the deployment
package (`deploy/compose.yaml`).

## Adding a journey

1. A new `tests/<journey>.spec.ts` importing `test` and `expect` from `../support/fixtures` (that gives it its own
   stack and `baseURL`); `test.describe.configure({ mode: "serial" })` when its tests build on each other.
2. Reach the state with the helpers of `support/app.ts` (`signIn`, `analyze`, `waitForState`, `finishedReview`,
   `openStep`, `navTo`); script the agent with `stack.script` before the analysis or question that needs it, and
   `test.skip(!stack.script, …)` so the file still runs against an external stack.
3. Call `checkScreen(page)` in every state the journey reaches (wording rules and axe).
4. A product defect found by a journey: keep the failing test as `test.fixme` with a one-line reason and report it.

Not drivable with the fakes (fixture PR and fake GitHub): binary files, files too large to show, line-ending
changes, review threads or installations "not checked", a review published on GitHub meanwhile
(`comment_published`).

## The stack by hand

```sh
deploy/dev/stack.sh            # prints SR_DEV_URL=...; open it and sign in; Ctrl+C stops everything
deploy/dev/stack.sh --help     # every input and every variable it sets
deploy/dev/stack-check.sh      # launcher checks: git isolation, relative state dir, cleanup on start timeout
deploy/dev/real-codex-smoke.sh # one analysis of a public PR with the real Codex provider (see its header)
deploy/dev/smoke-publish-check.sh # smoke publication checks (fake credentials): marker names, leaks, failed scans
```
