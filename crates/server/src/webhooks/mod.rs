//! Dedicated GitHub App webhook ingress and its in-process delivery worker.

mod changes;
mod drain;
mod receive;
mod worker;

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::Router;
use axum::error_handling::HandleErrorLayer;
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::middleware::from_fn;
use axum::routing::post;
use contracts::common::RepoRef;
use contracts::routes::GITHUB_WEBHOOK;
use contracts::time::Timestamp;
use db::repo::Db;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use tokio::net::TcpListener;
use tokio::sync::{Notify, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::Instant;
use tower::ServiceBuilder;
use tower::limit::ConcurrencyLimitLayer;
use tower::load_shed::LoadShedLayer;
use tower::timeout::TimeoutLayer;

use crate::config::Config;

use drain::{BODY_TIMEOUT, MAX_BYTES, drain_unread_body};

const HANDLERS: usize = 8;
const HEADER_TIMEOUT: Duration = Duration::from_secs(5);
const SIGNATURE_WARN_INTERVAL: Duration = Duration::from_secs(60);
const DROPPED_LOG_KEYS: usize = 2_000;

/// State shared by the webhook receiver and its single delivery worker.
#[derive(Clone)]
pub struct Webhooks {
    db: Db,
    config: Arc<Config>,
    wake: Arc<Notify>,
    security_log: Arc<Mutex<SecurityLog>>,
}

#[derive(Default)]
struct SecurityLog {
    last_signature_warning: Option<Instant>,
    signature_count: u64,
    drop_day: Option<Date>,
    dropped: HashSet<(Option<RepoRef>, Option<i64>)>,
}

impl Webhooks {
    /// Creates the receiver and worker over the given database and configuration.
    pub fn new(db: Db, config: Arc<Config>) -> Self {
        Self {
            db,
            config,
            wake: Arc::default(),
            security_log: Arc::default(),
        }
    }

    /// Starts the single delivery worker and reports whether webhook reception is configured.
    pub fn spawn(&self, stopped: watch::Receiver<bool>) -> Option<JoinHandle<()>> {
        if self.config.github_webhook_secret.is_none() {
            tracing::warn!(
                "GitHub webhook secret not set; repositories are not live, only Refresh and the nightly scan update My work"
            );
            return None;
        }
        Some(tokio::spawn(worker::run(self.clone(), stopped)))
    }

    fn signature_rejected(&self) -> Option<u64> {
        let mut log = self
            .security_log
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        log.signature_count = log.signature_count.saturating_add(1);
        let now = Instant::now();
        if log
            .last_signature_warning
            .is_some_and(|last| now - last < SIGNATURE_WARN_INTERVAL)
        {
            return None;
        }
        log.last_signature_warning = Some(now);
        Some(std::mem::take(&mut log.signature_count))
    }

    fn log_drop(&self, repo: Option<&RepoRef>, installation: Option<i64>, now: Timestamp) -> bool {
        let mut log = self
            .security_log
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let day = now.to_zoned(TimeZone::UTC).date();
        if log.drop_day != Some(day) {
            log.drop_day = Some(day);
            log.dropped.clear();
        }
        let key = repo.map(|repo| RepoRef {
            owner: repo.owner.to_ascii_lowercase(),
            name: repo.name.to_ascii_lowercase(),
        });
        let installation = installation.filter(|_| key.is_none());
        log.dropped.len() < DROPPED_LOG_KEYS && log.dropped.insert((key, installation))
    }
}

/// Builds the webhook-only router and shares a handler limit across all connections.
pub fn router(state: Webhooks) -> Router {
    let handler_layers = ServiceBuilder::new()
        .layer(HandleErrorLayer::new(|_: tower::BoxError| async {
            StatusCode::SERVICE_UNAVAILABLE
        }))
        .layer(LoadShedLayer::new())
        .layer(ConcurrencyLimitLayer::new(HANDLERS))
        .layer(TimeoutLayer::new(BODY_TIMEOUT))
        .layer(DefaultBodyLimit::max(MAX_BYTES));
    Router::new()
        .route(
            GITHUB_WEBHOOK.path,
            post(receive::receive).route_layer(handler_layers),
        )
        .layer(from_fn(drain_unread_body))
        .with_state(state)
}

/// Serves HTTP/1 with a timed header read and closes connections gracefully on shutdown.
pub async fn serve(mut listener: TcpListener, app: Router, mut stopped: watch::Receiver<bool>) {
    let mut connections = JoinSet::new();
    let mut http = hyper::server::conn::http1::Builder::new();
    http.timer(TokioTimer::new())
        .header_read_timeout(HEADER_TIMEOUT);
    loop {
        tokio::select! {
            biased;
            _ = stop(&mut stopped) => break,
            _ = connections.join_next(), if !connections.is_empty() => {}
            (stream, _) = axum::serve::Listener::accept(&mut listener) => {
                let service = TowerToHyperService::new(app.clone());
                let connection = http.serve_connection(TokioIo::new(stream), service);
                let mut stopped = stopped.clone();
                connections.spawn(async move {
                    let mut connection = std::pin::pin!(connection);
                    tokio::select! {
                        _ = &mut connection => {}
                        _ = stop(&mut stopped) => {
                            connection.as_mut().graceful_shutdown();
                            let _ = connection.await;
                        }
                    };
                });
            }
        }
    }
    drop(listener);
    while connections.join_next().await.is_some() {}
}

async fn stop(stopped: &mut watch::Receiver<bool>) {
    let _ = stopped.wait_for(|stop| *stop).await;
}

#[cfg(test)]
mod tests;
