//! Shared application state handed to every handler.

use std::sync::Arc;

use db::repo::Db;

use crate::auth::PendingPairs;
use crate::config::Config;
use crate::dashboard::DashboardHub;
use crate::dashboard::plugin::PluginHost;
use crate::jobs::executor::JobExecutor;
use crate::jobs::progress::ProgressHub;
use crate::jobs::queue::QueueExecutor;
use crate::review_loop::feed::FeedHub;
use crate::review_loop::notify::NotifyHub;
use crate::review_loop::seen::ReportReads;

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
    /// Per-user dashboard refreshes (ADR-017).
    pub dashboard: DashboardHub,
    /// Dashboard plug-ins (ADR-022); empty unless configured.
    pub plugins: PluginHost,
    /// Decision feed streams and their presence (ADR-023).
    pub feed: FeedHub,
    /// Coalesces reads after agent reports within this application (ADR-023).
    pub report_reads: ReportReads,
    /// Wakes the notification delivery task (ADR-023).
    pub notify: NotifyHub,
    /// Process-local webhook counters shared by both HTTP listeners.
    pub webhook_metrics: crate::webhooks::WebhookMetrics,
}

impl AppState {
    /// Starts application tasks after startup recovery.
    pub fn spawn_background(&self) {
        crate::dashboard::schedule::spawn(self.clone());
        crate::dashboard::plugin::spawn(self.clone());
        crate::review_loop::feed::spawn(self.clone());
        crate::review_loop::notify::spawn(self.clone());
    }

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
            plugins: PluginHost::from_config(&config),
            config: Arc::new(config),
            db,
            executor,
            progress: ProgressHub::default(),
            pending_pairs: PendingPairs::default(),
            stopping: tokio::sync::watch::Sender::new(false),
            dashboard: DashboardHub::default(),
            feed: FeedHub::default(),
            report_reads: ReportReads::default(),
            notify: NotifyHub::default(),
            webhook_metrics: Default::default(),
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

    #[tokio::test]
    async fn startup_after_container_migration_recovers_and_serves_preserved_settings() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use contracts::job::spec::ReasoningEffort;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        for populated in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("diffpilot.sqlite");
            if populated {
                db::repo::testing::pre_container_database(&path)
                    .await
                    .unwrap();
            }
            let db = Db::open(&path).await.unwrap();
            let mut config = test_config();
            config.data_dir = dir.path().to_owned();
            config.github.api_url = "http://127.0.0.1:9/".parse().unwrap();
            let state = AppState::with_queue(config, db);
            crate::auth::apply_allowed_logins(&state.db, &["alice".into()])
                .await
                .unwrap();
            assert_eq!(
                crate::pipeline::control::recover_cancellations(&state)
                    .await
                    .unwrap(),
                0
            );
            assert_eq!(crate::pipeline::resume_unfinished(&state).await.unwrap(), 0);
            assert_eq!(crate::ask::resume_unanswered(&state).await.unwrap(), 0);
            assert_eq!(crate::review_loop::assess::resume(&state).await.unwrap(), 0);
            crate::dashboard::refresh::recover(&state).await.unwrap();
            let router = crate::api::router(state.clone());
            let response = router
                .clone()
                .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
            if !populated {
                continue;
            }
            let selection = state.db.settings().llm_selection().await.unwrap().unwrap();
            assert_eq!(selection.choice.model, "gpt-6.2-sol");
            assert_eq!(selection.choice.reasoning_effort, ReasoningEffort::Low);
            assert_eq!(selection.ask_effort, ReasoningEffort::High);
            assert_eq!(selection.judge_effort, ReasoningEffort::High);
            for path in [
                "/api/v1/me",
                "/api/v1/settings",
                "/api/v1/settings/dashboard",
            ] {
                let response = router
                    .clone()
                    .oneshot(
                        Request::get(path)
                            .header("cookie", "__Host-sr_session=fake-migration-session")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK, "{path}");
                let body = response.into_body().collect().await.unwrap().to_bytes();
                let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
                if path == "/api/v1/settings/dashboard" {
                    assert_eq!(json["settings"]["time_zone"], "Europe/Warsaw");
                    assert_eq!(
                        json["settings"]["skip_authors"],
                        serde_json::json!(["skip-bot"])
                    );
                    assert_eq!(
                        json["settings"]["repositories"][0]["branch_patterns"],
                        serde_json::json!(["release/*"])
                    );
                    assert_eq!(
                        json["settings"]["repositories"][0]["ignored_checks"],
                        serde_json::json!(["optional"])
                    );
                }
            }
        }
    }
}
