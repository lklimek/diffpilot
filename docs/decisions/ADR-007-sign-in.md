# ADR-007: Sign-in and user tokens

- Status: accepted (coordinator, 2026-11-16, after spike S4; owner answer on the allowlist)
- Source: `docs/spikes/S4-app-tokens.md`; brief §2 Users; plan §16 Sign-in.

## Decisions
- **Web flow** with `state` and PKCE `S256`; exact-match callback URLs (wildcard matching disabled on the App);
  expiring user tokens on. The device flow is a development and spike tool only.
- **Refresh** before expiry, serialised per user; the rotated pair is stored encrypted (AES-GCM, ADR-001)
  atomically before the old pair is discarded; OAuth errors arrive as HTTP 200 with `error`;
  `bad_refresh_token` ends the session and the user signs in again.
- **Allowed logins** are a tenant setting without UI or configuration file. The seeded tenant `default` allows
  `lklimek` and `Claudius-Maginificent` (case-insensitive). The first successful sign-in binds the allowed login
  to the numeric GitHub user id (`tenant_allowed_logins.github_id`, set once); later sign-ins must match that id,
  so a renamed or re-registered login cannot take the entry over (threat model WP1-T).
  For the owner: [deviation] brief §2 says the seed allows exactly `lklimek`; the owner approved adding
  `Claudius-Maginificent` (2026-11-16).
- **Login errors** reach Home through the callback redirect (`/?login_error=<code>`, codes `login_not_allowed`,
  `oauth_failed`, `state_mismatch`).

## Addendum (WP3-AUTH, 2026-11-17): sessions, sign-out and token lifetime
- **Session lifetime**: 7 days absolute, 24 h idle (`auth::SESSION_TTL`, `SESSION_IDLE_TIMEOUT`); the brief sets
  none. A session is live only while its user's GitHub id is bound to an allowlist entry of the session's
  tenant, so removing a login (or rebinding its entry) ends access on the next request. Each request decides
  liveness by a read; the use is recorded (`last_seen_at`) only when the stored one is older than 60 s
  (`SESSION_TOUCH_INTERVAL`) and best-effort: a failed write (database busy) is logged and the request proceeds.
  The idle timeout therefore counts from a use up to 60 s (more after failed writes) before the last one, so a
  session may end that much early, never late; the write only updates a still-live session, so it never revives
  a signed-out session or extends an ended one.
- **Token pair lifetime**: the GitHub pair is kept only while the user has a live session. Sign-out of one
  session deletes the pair with the user's last live session; sign-out everywhere, a rejected refresh
  (`bad_refresh_token`), an expired refresh token and a pair that no longer decrypts (`SR_SECRET_KEY` replaced)
  delete the pair and every session. A sign-in writes its session and pair in one transaction (neither on
  failure). The cleanup at each sign-in start deletes dead sessions and pairs left without sessions.
- **GitHub 401**: a 401 for an access token the server still holds as fresh marks it expired; the next use
  refreshes the pair, and a rejected refresh ends the sign-in ("Sign in again"). Wired in Settings, the GitHub
  snapshot (`review`) and the pending-review comments; the pipeline's own user-token calls are not wired yet.
- **Refresh**: runs in its own task under the user's lock, so a dropped request keeps the rotated pair.
  Residual risk: a server crash or a lost GitHub answer (timeout, reset) between the rotation and the database
  write loses the new pair; the next refresh gets `bad_refresh_token` and the user signs in again.
