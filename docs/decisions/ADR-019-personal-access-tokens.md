# ADR-019: Personal access tokens — read-only API access for the user's agents

- Status: accepted (owner, 2026-11-18, OD17: "the agent is to see everything; it has those permissions anyway");
  plan `docs/v1.1-dashboard-plan.md` §E.2, §E.5, §F.6, WP11-TOKENS.

## Context
The owner's agents (e.g. the dispatcher skill) need the dashboard ranking and review content without a browser.
v1 authenticates the public API with a cookie session (+ CSRF for mutations) and the runner listener with bearer
secrets (runner secret, per-job token; `docs/contracts/http-api.md` "Listeners and authentication").

## Decision
1. **Token.** `srp_<id>_<secret>`: `id` = 8 random base62 characters (public, shown in Settings, used for lookup);
   `secret` = 32 random bytes (256 bits) base64url. Stored: `id`, SHA-256 of the whole token (the existing
   `secrets::sha256`, as v1 session tokens), name, scopes, `created_at`, `expires_at`, `last_used_at` (written at
   most once a minute), `revoked_at`. A slow KDF is not used: it protects low-entropy passwords against guessing;
   a 256-bit random value cannot be guessed, so SHA-256 preimage resistance is enough. The hash is compared in
   constant time (`secrets::secrets_equal`). Shown once at creation, never again.
2. **Scopes** are a list; v1.1 has one, `read`. The scope list is the extension point (e.g. `dashboard:refresh`).
3. **Lifecycle.** Created, listed and revoked in Settings by the signed-in user only (R8–R10, session; a token can
   neither list nor mint tokens). Name 1–60 characters, unique per user; expiry 30, 90 (default) or 365 days, no
   unlimited tokens; ≤10 live tokens per user. A token stops working when it expires or is revoked, when its user
   leaves the tenant's sign-in allow-list (checked per request, as sessions), and on "sign out everywhere" (which
   also revokes all tokens). An ordinary sign-out leaves tokens alone.
4. **What it reads.** Exactly what its user reads with a session: every `GET` route marked `read`
   (`RouteAuth::SessionOrToken(read)`): the dashboard (R1, R1b, R2b, R5, R6) and every v1 `GET` route with session
   auth — analysis summary, analysis events, Tour, lines, diff, review state, questions, feedback, pending review,
   Settings, repository settings. Tenant and user scoping are the session user's. **Never** on a mutating method,
   on R8 (token list) or on the runner listener. `GET /api/v1/me` (public) answers a token with its user and
   `csrf_token: null`. Server-sent event streams accept tokens; the 15 s heartbeat re-checks the token, so a
   revocation ends the stream within 15 s. Reads backed by the user's GitHub token (pending review, App
   installations in Settings) work while the server holds the user's token pair (ADR-020) and otherwise answer as
   v1 does without one.
5. **One central extractor.** A middleware on the public API router handles `Authorization: Bearer srp_…`: a request
   with both a session cookie and a bearer token → 400 `invalid_request`; a route not marked `read` or a non-safe
   method → 403 `token_not_accepted`; unknown, malformed, expired, revoked or unlisted → 401 `unauthenticated` with
   `WWW-Authenticate: Bearer` (no reason given); valid but missing the route's scope → 403 `insufficient_scope`.
   On success it inserts a `TokenPrincipal`; the existing session extractor yields the principal from the cookie or
   from that extension, so v1 handlers are unchanged. `SessionCsrf` never consults it. A contract test over
   `ROUTES` asserts: every `GET` with session auth is `read` except R8; no non-`GET` route is.
6. **Never confused with runner bearers.** Different prefix (`srp_`), different listener (public vs runner),
   different extractors (the runner extractor never reads `access_tokens` and rejects `srp_` values early), different
   route groups (`/api/v1/*` vs `/runner/v1/*`). A runner secret or job token sent to the public API is an unknown
   token (401).
7. **Abuse limits and audit.** Per token 120 requests/min, per client address 30 failed bearer attempts/min
   (trusted-proxy address, as v1) → 429 `rate_limited` with `Retry-After`. One `info` line on target `audit` per
   token request (token id, user id, method, route template, status, duration) and per create/revoke; the token,
   its hash and query strings are never logged.

## v1 GET routes under a token
| Route | Note |
|---|---|
| `/api/v1/analyses/{id}`, `/events`, `/tour`, `/lines`, `/diff`, `/repos/{o}/{n}/settings` | tenant-shared analysis data, as for any session user; reader projections (`has_my_state`, `mine`) use the principal |
| `/review-state`, `/questions` | the principal's own (private Ask per user) |
| `/feedback` | **attention**: must list only the principal's feedback (verify the filter in WP11-TOKENS; fix if tenant-wide) |
| `/pending-review` | the principal's GitHub pending review, via the stored user token |
| `/api/v1/settings` | installation LLM choice shows the login of the member who changed it — as for sessions; no secrets |
v1 has no list of analyses; agents reach analyses through dashboard items' `review` links. A list route is a later
additive change.

## Threat model (T20, AR-10)
A token is as powerful as a read-only session of its user — private-repository titles, Tour text, Q&A, pending
comments — and lives outside the browser, in the user's agents' environments and files; it has no CSRF exposure
(no ambient credential). Mitigations: expiry, revocation, allow-list check, rate limits, audit, hash at rest.
Accepted by the owner (AR-10): his agents already hold his GitHub access.

## Alternatives considered
| Alternative | Why not |
|---|---|
| Per-route token checks in handlers | easy to miss a route; the central middleware + `ROUTES` contract test cannot |
| GitHub tokens as API credentials | user tokens never leave the server (T3) |
| Argon2/bcrypt hashing | no benefit for 256-bit random secrets; costs CPU on every request |
| Tokens accepted on mutating routes with CSRF exemption | not wanted now; a write scope can be added later per route (plan §E.5 E5-8) |
