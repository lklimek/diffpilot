//! Helpers shared by the server's unit tests.

pub(crate) mod github;
pub(crate) mod logs;
pub(crate) mod pr_hygiene;

use contracts::time::Timestamp;
use db::repo::sessions::NewSession;
use jiff::SignedDuration;

use crate::secrets::sha256;
use crate::state::AppState;

/// A session of user `u1` (seeded with the analysis, GitHub id 1), allowed in `tenant`; returns its `Cookie`
/// header value.
pub(crate) async fn session_cookie(state: &AppState, tenant: &str) -> String {
    let token = format!("session-token-{tenant}");
    let now = Timestamp::now();
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
                expires_at: now + SignedDuration::from_hours(1),
            },
            now,
        )
        .await
        .expect("session");
    // The test configuration's public URL is https, so the cookie carries the `__Host-` prefix.
    format!("__Host-sr_session={token}")
}

/// A second authenticated reader in the same tenant, for privacy regression tests.
pub(crate) async fn other_user_cookie(state: &AppState) -> String {
    use db::repo::{DEFAULT_TENANT, users::GithubProfile};
    let now = Timestamp::now();
    db::repo::tenants::testing::allow_login(&state.db, DEFAULT_TENANT, "colleague", 2)
        .await
        .expect("allow");
    state
        .db
        .users()
        .upsert_member(
            DEFAULT_TENANT,
            "u2",
            &GithubProfile {
                github_id: 2,
                login: "colleague".to_owned(),
                name: None,
                avatar_url: None,
            },
            now,
        )
        .await
        .expect("user");
    state
        .db
        .sessions()
        .create(
            &NewSession {
                token_hash: &sha256(b"private-user-two"),
                tenant_id: DEFAULT_TENANT,
                user_id: "u2",
                csrf_token: "csrf",
                expires_at: now + SignedDuration::from_hours(1),
            },
            now,
        )
        .await
        .expect("session");
    "__Host-sr_session=private-user-two".to_owned()
}
