# e2e — full-stack Playwright

The v1 user journeys through the real stack, no mocks in the browser: server with the embedded web UI, runner
with the fake provider (fixture replay of `fixtures/pr-482`, scriptable), fake GitHub (`tools/fake-github`) and
fake MemCan (`tools/fake-memcan`). Every reached state is checked by `checkScreen` (`support/app.ts`): the wording rules of brief §1
(`scripts/wording-rules.json`, shared with `scripts/check-wording.sh`) on the page text, hidden tooltips included,
no `%` at all in the top bar, every tooltip opened by keyboard focus (aria-disabled controls included) and closed
when focus leaves (its text checked too), the controls each screen must give a tooltip (`REQUIRED_TIPS`: top bar,
quiet rows, risk and provenance labels, Previous/Next, feedback values, Ask, Check, Cancel/Retry) present with a
non-empty tip, no `aria-describedby` naming a missing element, and axe (serious or critical = failure). When a local stack stops, its component logs are checked for leaked secrets
(`support/secrets.ts`): generated secrets, the App private key lines, every App JWT and installation token fake
GitHub issued, fixture credentials, session secrets the tests registered (personal access tokens they created, the
judge job's input and output text), anything shaped like the server's random tokens (any 43-character base64url run;
job tokens are known only by that shape: the server keeps just their hash) and any whole personal access token
(`srp_<id>_<secret>`). A leak fails the run.

| Spec                              | Journey                                                                                                               |
| --------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `check-screen.spec.ts`            | self-test of `checkScreen` on static pages (no stack): it fails on each kind of violation                             |
| `secrets.spec.ts`                 | self-test of the leak check's token shape (no stack): any 43-character base64url run, known non-secrets excluded      |
| `slice.spec.ts`                   | sign in, analyse PR 482, flow 1 step 1; a read-only installation token per job, revoked; no secret in the logs so far |
| `analysis-cancel.spec.ts`         | planning, cancel → "Check the PR to start again"; failure before any flow → Retry → `k of n` → finished                             |
| `analysis-cancel-running.spec.ts` | cancel while a flow job runs: the job stops, stored flows and session discarded, a clean new analysis                 |
| `analysis-pause.spec.ts`          | usage limit → "Paused until HH:MM" of `resumes_at`, no attempt before it, one after; failed flow → Retry this flow    |
| `tour.spec.ts`                    | PR context facts, flow order, quiet rows, no LLM flags (hard case 3), classic diff, resume, columns               |
| `flow-groups.spec.ts`             | a flow the LLM classifies as refactor: after the behaviour flows, labelled; tests listed once, not as a flow          |
| `comments.spec.ts`                | pending review: create, second comment, edit, delete, outside the diff, missed by the LLM (hard case 2)               |
| `head-moved.spec.ts`              | `pr_head_changed`, "The PR has moved" → Start analysis without ticks, Ask on the superseded analysis still answered   |
| `feedback.spec.ts`                | wrong + overrated → MemCan calibration example (hard case 1), removal, MemCan failures, recovery                      |
| `ask.spec.ts`                     | answered and pinned, usage-limit wait until `resumes_at`, failed answer, Ask again, markup as text                    |
| `settings.spec.ts`                | signed-out screens, LLM status after a failed and a working call, language, Sign out                                  |
| `security.spec.ts`                | CSP header; an inline script does not run; LLM analysis text with markup renders as text                              |
| `dashboard.spec.ts`               | My work: Refresh → scan steps → provisional → ranked, tab title busy → ready; TODOs off; Review link, `/?pr=`; cancel scan; MemCan down |
| `dashboard-ranking.spec.ts`       | the ranking waits for a running review analysis; a cancelled ranking → Retry ranking                                  |
| `dashboard-failures.spec.ts`      | GitHub rate limit: failed with "Retry at HH:MM", 429 until then; GitHub down: a plain message; the previous list stays |
| `dashboard-text.spec.ts`          | hostile TODO and LLM text (markup, markdown and `javascript:` links) renders as text (`dashboard_llm_text_renders_as_text`) |
| `dashboard-views.spec.ts`         | Excluded disclosure with its reason; a PR waiting only on others; size pills and Sizes shown; kind toggles kept over a reload; Recent reviews |
| `dashboard-language.spec.ts`      | Polish chosen: every ranking job's prompt asks for Polish                                                             |
| `dashboard-settings.spec.ts`      | Dashboard settings validation and save, ranking effort; access token: bearer reads, writes 403, revoked 401; second user |

## Run

Requirements: Rust toolchain, Node 22, `git`, `curl`, `openssl`, `python3`, `sqlite3`.

```sh
(cd web && npm ci)                  # the global setup builds web/dist and the binaries
cd e2e
npm ci
npx playwright install chromium     # once per machine
npx playwright test                 # one run at a time; at most 2 workers
npx playwright test tests/ask.spec.ts
npx tsc --noEmit                    # type-check the suite
```

The global setup builds once (`npm run build` in `web/`, then `cargo build` of server with `embed-ui`, runner with
`fake-provider`, `fake-github`, `fake-memcan`). Prebuilt binaries skip the build, e.g. on a host where builds go
through a wrapper: set all four of `SR_DEV_SERVER_BIN`, `SR_DEV_RUNNER_BIN`, `SR_DEV_FAKE_GITHUB_BIN`,
`SR_DEV_FAKE_MEMCAN_BIN`.

| Variable                  | Effect                                                                                    |
| ------------------------- | ----------------------------------------------------------------------------------------- |
| `SR_E2E_OUTPUT_DIR`       | Traces and screenshots of failures (default `test-results`).                              |
| `SR_E2E_STACK_ARGS`       | Extra launcher arguments, e.g. `--without runner` (analyses then never finish).           |
| `SR_E2E_START_TIMEOUT_MS` | Bound for one stack start (default 2 min).                                                |
| `SR_E2E_BUILD_TIMEOUT_MS` | Bound for the global setup's build (default 30 min); the build's process group is killed. |
| `SCREENSHOT_DIR`          | `slice.spec.ts` and the dashboard specs save their screens at 1440 and 3440 px, light and dark. |
| `RUST_LOG`                | Log level of the components (default `info`).                                             |

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
  (`GET /_fake/state`) and its mode (`normal`, `embedding_failure` as MemCan fails, `synthetic_tool_error`,
  `unavailable`, `hang`, `delay`; `tools/fake-memcan/README.md`).
- `stack.githubCredentials()`: App JWTs accepted and installation tokens minted and revoked by fake GitHub.
- `runnerEvents(stack)`, `sql(stack, query)` (`support/app.ts`): the runner's job attempts from its log, read-only
  queries on the server database.
- The stack's launcher is tied to its worker: a guard process stops it when the worker dies, also on SIGKILL.
- `stack.github`: fake GitHub's URL for its test controls: dashboard faults (`githubFaults`) and the user of later
  sign-ins (`nextGithubUser`, with `allowLogin` for the server's allow-list) in `support/dashboard.ts`, which also
  holds the dashboard world's settings (`worldSettings`) and fake MemCan's TODOs (`seedTodos`). The world was
  written for 2026-11-18T19:30:00Z; the launcher's fake GitHub moves its dates so that instant is its start-up
  (`SR_DEV_DASHBOARD_WORLD_AT`, `GET /_fake/dashboard/clock`) and `seedTodos` dates the TODOs the same way, so the
  ranked order the specs expect holds on any day, with no clock in the server.
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
of PR 482 is reused by the later files.

CI (`.github/workflows/ci.yml`, job `e2e`) builds the web UI and the four binaries once, runs
`deploy/dev/stack-check.sh`, then the whole suite with 2 workers on local dev stacks; on failure it uploads the
Playwright traces and each stack's `logs/` (never `stack.env` or the App key).

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
deploy/dev/stack-check.sh      # git isolation, relative state dir, build timeouts, a killed worker leaves no stack
deploy/dev/real-codex-smoke.sh # one analysis of a public PR with the real Codex provider (see its header)
deploy/dev/smoke-publish-check.sh # smoke publication checks (fake credentials): marker names, leaks, failed scans
```
