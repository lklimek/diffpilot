//! Shared application state handed to every handler.

use std::sync::Arc;

use db::repo::Db;

use crate::auth::PendingPairs;
use crate::config::Config;
use crate::jobs::executor::JobExecutor;
use crate::jobs::progress::ProgressHub;
use crate::jobs::queue::QueueExecutor;

/// State shared by all handlers; cheap to clone.
#[derive(Clone, Debug)]
pub struct AppState {
    /// Process-local numeric observations for terminal statistics.
    pub statistics: crate::pipeline::statistics::Statistics,
    /// Server configuration.
    pub config: Arc<Config>,
    /// The database.
    pub db: Db,
    /// Runs agent jobs (v1: `QueueExecutor`).
    pub executor: Arc<dyn JobExecutor>,
    /// Per-analysis progress channels behind the SSE stream (CCR-J1).
    pub progress: ProgressHub,
    /// Refreshed GitHub token pairs not stored yet because the database was busy.
    pub pending_pairs: PendingPairs,
    /// `true` once the server stops: open event streams end so the listeners can finish.
    pub stopping: tokio::sync::watch::Sender<bool>,
}

impl AppState {
    /// The state with the database queue as executor, its followers publishing to the state's progress hub.
    pub fn with_queue(config: Config, db: Db) -> Self {
        let progress = ProgressHub::default();
        let executor = Arc::new(QueueExecutor::new(db.clone(), progress.clone()));
        Self {
            progress,
            ..Self::new(config, db, executor)
        }
    }

    /// Builds the state from a loaded configuration, the opened database and the job executor.
    pub fn new(config: Config, db: Db, executor: Arc<dyn JobExecutor>) -> Self {
        Self {
            statistics: crate::pipeline::statistics::Statistics::default(),
            config: Arc::new(config),
            db,
            executor,
            progress: ProgressHub::default(),
            pending_pairs: PendingPairs::default(),
            stopping: tokio::sync::watch::Sender::new(false),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::tests::test_config;

    /// State over an in-memory database and the queue executor, for handler tests.
    pub(crate) async fn test_state() -> AppState {
        let db = Db::in_memory().await.expect("in-memory database");
        AppState::with_queue(test_config(), db)
    }

    #[tokio::test]
    async fn state_holds_config_database_and_executor() {
        let state = test_state().await;
        assert_eq!(state.config.listen_addr.port(), 3000);
        let rendered = format!("{state:?}");
        assert!(rendered.contains("QueueExecutor"), "{rendered}");
    }
}
