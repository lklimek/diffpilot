# fake-github

Development and test only (never in images): a fake GitHub serving the recordings of
`fixtures/pr-482/github` for the example PR `acme/node-client#482` and, optionally, the synthetic dashboard
world of `fixtures/dashboard/github` (see [Dashboard world](#dashboard-world)). Deterministic; never contacts
GitHub.

## Run

```sh
cargo run -p fake-github -- \
  --fixture-dir fixtures/pr-482/github \
  --app-url http://localhost:3000
```

| Flag | Env | Default | Meaning |
|---|---|---|---|
| `--listen` | `FAKE_GITHUB_LISTEN` | `127.0.0.1:3900` | Listen address. |
| `--fixture-dir` | `FAKE_GITHUB_FIXTURE_DIR` | — | Recordings directory; the tool refuses to start if it or a recording is missing. |
| `--app-url` | `FAKE_GITHUB_APP_URL` | — | The server's `SR_PUBLIC_URL`; `<app-url>/auth/github/callback` is the registered callback URL. |
| `--user-login` | `FAKE_GITHUB_USER_LOGIN` | recorded (`alice`) | Login of the fake user. |
| `--user-id` | `FAKE_GITHUB_USER_ID` | recorded | Numeric id of the fake user. |
| `--app-id` | `FAKE_GITHUB_APP_ID` | — | GitHub App id the App JWTs must name (`SR_GITHUB_APP_ID`). |
| `--app-public-key-file` | `FAKE_GITHUB_APP_PUBLIC_KEY_FILE` | — | PEM public key of the App key (`openssl rsa -pubout`); App JWTs must verify against it. |
| `--dashboard-fixture-dir` | `FAKE_GITHUB_DASHBOARD_FIXTURE_DIR` | — | Dashboard world (`fixtures/dashboard/github`); without it the dashboard endpoints answer 404. |
| `--dashboard-world-at` | `FAKE_GITHUB_DASHBOARD_WORLD_AT` | — | The instant the world was written for (RFC 3339): every date of the world moves by the time from it to start-up, so the world is always as recent as on that day. Without it the dates are served as written. |

Log level: `RUST_LOG` (default `info`).

Server configuration against it (one listener serves OAuth and API):

```sh
SR_GITHUB_OAUTH_URL=http://127.0.0.1:3900
SR_GITHUB_API_URL=http://127.0.0.1:3900
SR_GITHUB_CLIENT_ID=fixture-app-client-id        # from oauth/access-token.json
SR_GITHUB_CLIENT_SECRET=fixture-client-secret    # from oauth/access-token.json
SR_GITHUB_CLONE_URL=file://<clone base>          # see below
```

## Endpoints

| Endpoint | Behaviour |
|---|---|
| `GET /login/oauth/authorize` | Signs the fake user in at once: `302` to the callback with `code` and `state`. Unknown `client_id` → 404; `redirect_uri` other than the callback → `302` to the callback with `error=redirect_uri_mismatch`; PKCE: `code_challenge` with `code_challenge_method=S256`, or neither (400 otherwise). |
| `POST /login/oauth/access_token` | Form body, `Accept: application/json` required (406 otherwise). Code exchange checks client credentials, `redirect_uri` and the PKCE verifier; codes are single-use (a wrong verifier leaves the code usable). `grant_type=refresh_token` rotates the pair and invalidates the old access token. Errors are HTTP 200 with `error` (`incorrect_client_credentials`, `bad_verification_code`, `bad_refresh_token`, `redirect_uri_mismatch`), as on GitHub (spike S4). |
| `GET /user` | The recorded user (login/id overridable) for a valid `Bearer`/`token` access token; 401 `Bad credentials` otherwise. |
| Recorded REST (`RECORDED_ENDPOINTS` in `src/fixtures.rs`) | The recording's `response` byte for byte with its `status`, matched by method and path (the query is ignored). PR, commits, files and head check runs of `acme/node-client#482`. A present `Authorization` must be a valid access token (401 `Bad credentials` otherwise); anonymous reads are served. |
| `GET /repos/{owner}/{repo}/installation` | With an App JWT checked like GitHub (RS256 signature against `--app-public-key-file`, `iss` = `--app-id`, `iat` not in the future, `exp` in the future and at most 10 minutes ahead; 401 with GitHub's message otherwise, and always without both flags): `rest/repo-installation.json` for the recorded repository, `rest/repo-installation-missing.json` (404) otherwise. |
| `POST /app/installations/{id}/access_tokens` | With an App JWT and an explicit read-only scope (`repositories` = the recorded repository only, `permissions` all `read` and granted to the installation; 422 otherwise, also for a missing scope, which GitHub would widen): a unique token (`<recorded token>-<n>`) carrying the requested permissions, `expires_at` one hour from now (201); 404 for another installation. |
| `DELETE /installation/token` | 204 for a minted, not yet revoked installation token, 401 otherwise. |
| `GET /_fake/app-credentials` | Test control, no authentication: `{jwts, installation_tokens, revoked}` accepted and issued so far (log scans). |
| `POST /_fake/user` | Test control, no authentication: `{"login": "bob", "id": 77}` makes later sign-ins (code exchanges) that user: `GET /user` answers the recorded profile with this login and id for their tokens, also after a refresh. Tokens issued before keep their user. |
| `POST /_fake/head` | Test control, no authentication: `{"sha": "<40 hex>"}` becomes the pull request's head in `rest/pull.json` answers and GraphQL (`headRefOid`, new comments' commit), as after a push. The commit must exist in the clone base for an analysis of it. |
| `POST /graphql` | Valid access token required (401 otherwise). Recognises the server's documents by the fields they use: review threads (`graphql/review-threads.json` plus the user's pending threads), the pull request's node id and head (`rest/pull.json`), and the user's pending review kept in memory — `addPullRequestReviewThread` creates the review or reuses it and answers `thread: null` when the file is not in `rest/pull-files.json` or a line thread's lines are not in one hunk of their side (as GitHub, spike S3); `updatePullRequestReviewComment`, `deletePullRequestReviewComment` (`NOT_FOUND` for an unknown id; the review disappears with its last comment). Creating or submitting a review is refused (`FORBIDDEN`). |
| Anything else | 404 `Not Found` and a `WARN fake-github has no recording …` log line. |

REST requests without `User-Agent` get 403, as on GitHub.

Tokens and codes are deterministic: the first code and token pair are the recorded values
(`fixture-oauth-code`, `fixture-user-token-alice`, `fixture-refresh-token-alice`), later ones append `-2`, `-3`, …

Adding a recorded REST endpoint: put the `{request, status, response}` file under the fixture dir and add its
path to `RECORDED_ENDPOINTS`.

## Clone base

The fake serves no git protocol. The dev stack clones from a `file://` base:

```sh
tools/fake-github/prepare-clone-base.sh /data/tmp/fake-github-remotes
SR_GITHUB_CLONE_URL=file:///data/tmp/fake-github-remotes
```

The script builds the fixture repository with `fixtures/pr-482/repo/build-repo.sh` and mirrors it to
`<DEST>/acme/node-client.git` with all refs (`main`, `feat/retry-timeouts`, `refs/pull/482/head`; commit ids as
in `fixtures/pr-482/repo/commits.json`).

CI recordings include successful, failed, skipped and neutral check runs plus a pending commit status at
`GET /repos/acme/node-client/commits/{head}/status`. This lets callers distinguish both CI sources and avoid
reporting skipped runs as passing. The existing fixture bearer-token checks apply to both endpoints.

## Dashboard world

With `--dashboard-fixture-dir fixtures/dashboard/github` the fake also answers the dashboard scan (plan
`docs/v1.1-dashboard-plan.md` §A.2, §A.3) from a synthetic world: `accounts.json` (logins; `"searchable": false`
makes search refuse one), `repositories.json`, `pull-requests.json` and `issues.json` (GraphQL nodes; loading
fills `__typename`, `id`, `url`, `repository`, `baseRepository` and empty connections), `rest.json` (releases,
milestones, contents, pull request files keyed `owner/name#n`, user events). Recorded endpoints win over the
world (`/repos/acme/node-client/pulls/482/files` stays the recording); the world's `acme/node-client#482`
follows `POST /_fake/head`. Private repositories need a valid token; a present invalid token gets 401,
a missing `User-Agent` 403.

| Endpoint | Behaviour |
|---|---|
| `GET /search/issues` | Qualifiers of Q1, Q10, Q12: `is:pr`/`is:pull-request`/`type:pr` or `is:issue` (required, else 422), `is:open`/`is:closed`/`state:`, `archived:`, `author:`, `assignee:`, `review-requested:`, `reviewed-by:`, `mentions:` (body and comments), `repo:` (OR-ed), `created:`/`updated:`/`closed:`/`merged:` with `>=D`, `>D`, `<=D`, `<D`, `D`, `A..B`, `*` (dates or RFC 3339, UTC). Other terms: 422 naming the term. An unknown, unsearchable or invisible login/repository: 422 "cannot be searched". Order `updated` desc (`sort=created`, `order=asc` honoured); `per_page` ≤ 100, `Link` header, 422 beyond the first 1000 results. |
| `GET /repos/{o}/{r}/releases/latest`, `/releases` | Latest = newest non-draft, non-prerelease (404 without one); list paginated. |
| `GET /repos/{o}/{r}/milestones` | `state=open` (default), `closed`, `all`; paginated. |
| `GET /repos/{o}/{r}/contents/{path}` | `Accept: application/vnd.github.raw[+json]` → raw text; else GitHub's base64 JSON. |
| `GET /repos/{o}/{r}/pulls/{n}/files` | Paginated (404 for an unknown pull request). |
| `GET /users/{u}/events` | Paginated; 422 beyond 300 events, as GitHub. |
| `POST /graphql` | Only what v1 sends reaches the v1 endpoint above: mutations, `node` lookups, and one unaliased `repository` lookup of the recorded pull request alone. Every other document is answered by the world; A valid token is checked before parsing, `User-Agent` after routing. Refused as a whole, before execution, like GitHub (HTTP 200, `errors` only, no `data`): a parse error (including an empty selection set `{ }`), nesting deeper than 64, a missing required or wrongly typed variable (variable definitions and defaults are honoured), an undeclared variable, an undefined or looping fragment, more than 20,000 selections after fragment expansion (each field, spread and inline fragment counts), a field the type does not have, an argument the field does not take or the fake does not implement (`orderBy`, `before`, …), a wrongly typed or missing required argument, and the pagination errors (`first`/`last` 1–100 required when `nodes`/`edges` are selected, invalid cursors). Validation is static, against a type map built from the fixtures: an object with `__typename` has that type, any other object the pseudo-type `<parent type>.<field>` (e.g. `PullRequest.milestone`, `PullRequest.labels.nodes`), plus the resolver fields, connections and `RateLimit`; a type's fields are the keys seen on it. So a typo is refused whatever the data (empty connections, `null` lookups), and a real GitHub field the fixtures never carry is refused until a fixture node has it. Root: `repository(owner, name)` (null + `NOT_FOUND` next to the data when invisible), `repositoryOwner(login)` (null without error), `rateLimit { cost remaining resetAt limit used }`. Repository: `pullRequest(number)`, `issue(number)` (null + `NOT_FOUND`), `pullRequests`/`issues(states, baseRefName, headRefName, first, last, after)`. Owner: `repositories(first, after, isArchived)`. Pull request: `reviews(author, states, first, last, after)`, `timelineItems(itemTypes)`. Fragments, inline fragments (by `__typename`) and variables work; only selected fields are answered. Cost per request `max(1, n + n/5)` for `n` `pullRequest` lookups (S11-2: 10 → 12); 30–49 lookups answer an HTML 502 page, 50+ a 504 (S11-2). |
| `POST /_fake/dashboard/faults` | Test control: `{"faults": [{"on": "search" \| "rest" \| "graphql", "matching": "<substring>", "times": 1, "answer": {...}}]}` queues injected answers (204). `matching` is checked against `path k=v&…` (decoded query) or the GraphQL document plus its variables. Answers: `{"kind": "secondary_rate_limit", "retry_after": 60, "status": 403}` (both optional; 403 or 429, secondary-limit message), `{"kind": "primary_rate_limit", "reset": <epoch>, "status": 429}` (`status` optional: REST 403 by default, 403 or 429; GraphQL 200 with `RATE_LIMITED` by default, or 403/429; `x-ratelimit-remaining: 0`, `x-ratelimit-reset`), `{"kind": "server_error", "status": 502, "html": true}`. |
| `GET /_fake/dashboard/clock` | Test control: `{"world_at", "shift_seconds"}`: how far the world's dates moved (0 without `--dashboard-world-at`); a test dates its own data (fake MemCan TODOs) the same way. |
| `GET /_fake/dashboard/requests` | Test control: every dashboard request in order of arrival: `seq` (assigned on arrival), `at_ms` (tokio clock since start, so paused test time works in-process), `on`, `method`, `path`, `query`, `q` (decoded search), GraphQL `lookups` (top-level fields without `rateLimit`), `pull_requests`, `cost`, `rate_limit_selected`, `status`, `fault` (`secondary_rate_limit`, `primary_rate_limit`, `server_error`, `oversized_batch`). |

Every dashboard answer carries `x-ratelimit-{limit,remaining,used,reset,resource}`: `search` 30 per 60 s,
`core` and `graphql` 5000 per hour, counted from the fake's start. The limits, including search's 30 per
minute, are never enforced by counting: only an injected fault answers a rate limit, so pacing is asserted on
the request log's `at_ms`. Search results without `sort` come in `updated` order (newest first), not GitHub's
best match; a client must not rely on that order. Dates in the world are fixed (around 2026-11-18): in-process tests
pass a fixed "now"; a fake started with `--dashboard-world-at` (the dev stack, the full-stack tests) moves them.
