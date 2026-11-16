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
