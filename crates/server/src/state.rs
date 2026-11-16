//! Shared application state handed to every handler.

use std::sync::Arc;

use db::repo::Db;

use crate::config::Config;
use crate::jobs::executor::JobExecutor;
use crate::jobs::progress::ProgressHub;

/// State shared by all handlers; cheap to clone.
#[derive(Clone, Debug)]
pub struct AppState {
    /// Server configuration.
    pub config: Arc<Config>,
    /// The database.
    pub db: Db,
    /// Runs agent jobs (v1: `QueueExecutor`).
    pub executor: Arc<dyn JobExecutor>,
    /// Per-analysis progress channels behind the SSE stream (CCR-J1).
    pub progress: ProgressHub,
}

impl AppState {
    /// Builds the state from a loaded configuration, the opened database and the job executor.
    pub fn new(config: Config, db: Db, executor: Arc<dyn JobExecutor>) -> Self {
        Self {
            config: Arc::new(config),
            db,
            executor,
            progress: ProgressHub::default(),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::tests::test_config;
    use crate::jobs::queue::QueueExecutor;

    /// State over an in-memory database and the queue executor, for handler tests.
    pub(crate) async fn test_state() -> AppState {
        let db = Db::in_memory().await.expect("in-memory database");
        let executor = Arc::new(QueueExecutor::new(db.clone()));
        AppState::new(test_config(), db, executor)
    }

    #[tokio::test]
    async fn state_holds_config_database_and_executor() {
        let state = test_state().await;
        assert_eq!(state.config.listen_addr.port(), 3000);
        let rendered = format!("{state:?}");
        assert!(rendered.contains("QueueExecutor"), "{rendered}");
    }
}
