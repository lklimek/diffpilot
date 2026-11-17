//! Helpers shared by the server's unit tests.

pub(crate) mod logs;

use chrono::{TimeDelta, Utc};
use db::repo::sessions::NewSession;

use crate::secrets::sha256;
use crate::state::AppState;

/// A session of user `u1` (seeded with the analysis, GitHub id 1), allowed in `tenant`; returns its `Cookie`
/// header value.
pub(crate) async fn session_cookie(state: &AppState, tenant: &str) -> String {
    let token = format!("session-token-{tenant}");
    let now = Utc::now();
    db::repo::tenants::testing::allow_login(&state.db, tenant, "lklimek", 1)
        .await
        .expect("allowlist");
    state
        .db
        .sessions()
        .create(
            &NewSession {
                token_hash: &sha256(token.as_bytes()),
                tenant_id: tenant,
                user_id: "u1",
                csrf_token: "csrf",
                expires_at: now + TimeDelta::hours(1),
            },
            now,
        )
        .await
        .expect("session");
    // The test configuration's public URL is https, so the cookie carries the `__Host-` prefix.
    format!("__Host-sr_session={token}")
}
