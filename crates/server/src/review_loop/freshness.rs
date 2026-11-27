//! Coverage and reader presence for webhook-driven Comments reads.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use contracts::common::RepoRef;
use db::error::DbError;
use tokio::time::Instant;

use super::READ_INTERVAL;
use crate::auth::Session;
use crate::dashboard::installations;
use crate::state::AppState;

/// Recently polled Comments pages, indexed by their tenant's stored PR id.
#[derive(Clone, Debug, Default)]
pub(crate) struct CommentsReaders(Arc<Mutex<HashMap<i64, (String, Instant)>>>);

impl CommentsReaders {
    fn active(at: Instant) -> bool {
        at.elapsed() <= READ_INTERVAL.unsigned_abs() * 2
    }

    /// Renews presence for two poll intervals, returning true when a browser reader becomes active.
    pub(crate) fn touch(&self, session: &Session, pr: i64) -> bool {
        if session.is_access_token() {
            return false;
        }
        let mut readers = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        readers.retain(|_, (_, at)| Self::active(*at));
        readers
            .insert(pr, (session.user_id.clone(), Instant::now()))
            .is_none()
    }

    /// Most recent active browser reader of the PR.
    pub(super) fn reader(&self, pr: i64) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&pr)
            .filter(|(_, at)| Self::active(*at))
            .map(|(user, _)| user.clone())
    }
}

/// Whether this tenant's repository receives webhook updates.
pub(super) async fn covered(
    state: &AppState,
    tenant: &str,
    repo: &RepoRef,
) -> Result<bool, DbError> {
    Ok(installations::live(state, tenant)
        .await?
        .contains(&installations::key(repo)))
}
