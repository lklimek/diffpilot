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
| `GET /login/oauth/authorize` | Signs the fake user in at once: `302` to the callback with `code` and `state`. Unknown `client_id` → 404; `redirect_uri` other than the callback → `302` to the callback with `error=redirect_uri_mismatch`; PKCE only `S256`. |
| `POST /login/oauth/access_token` | Form body, `Accept: application/json` required (406 otherwise). Code exchange checks client credentials, `redirect_uri` and the PKCE verifier; codes are single-use (a wrong verifier leaves the code usable). `grant_type=refresh_token` rotates the pair and invalidates the old access token. Errors are HTTP 200 with `error` (`incorrect_client_credentials`, `bad_verification_code`, `bad_refresh_token`, `redirect_uri_mismatch`), as on GitHub (spike S4). |
| `GET /user` | The recorded user (login/id overridable) for a valid `Bearer`/`token` access token; 401 `Bad credentials` otherwise. |
| Recorded REST (`RECORDED_ENDPOINTS` in `src/fixtures.rs`) | The recording's `response` byte for byte with its `status`, matched by method and path (the query is ignored). PR, commits and files of `acme/node-client#482`. |
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
