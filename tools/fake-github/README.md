# fake-github

Development and test only (never in images): a fake GitHub serving the recordings of
`fixtures/pr-482/github` for the example PR `acme/node-client#482`. Deterministic; never contacts GitHub.

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
