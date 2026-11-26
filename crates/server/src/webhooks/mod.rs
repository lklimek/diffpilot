//! Dedicated GitHub App webhook ingress and its in-process delivery worker.

mod changes;
pub(crate) mod dispatch;
mod drain;
mod receive;
mod worker;

#[cfg(test)]
pub(crate) use worker::work_step;

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
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
use jiff::SignedDuration;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use tokio::net::TcpListener;
use tokio::sync::{Notify, Semaphore, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::Instant;
use tower::ServiceBuilder;
use tower::limit::ConcurrencyLimitLayer;
use tower::load_shed::LoadShedLayer;
use tower::timeout::TimeoutLayer;

use crate::config::Config;
use crate::state::AppState;

use drain::{BODY_TIMEOUT, MAX_BYTES, drain_unread_body};

const HANDLERS: usize = 8;
const CONNECTIONS: usize = 64;
const HEADER_TIMEOUT: Duration = Duration::from_secs(5);
const WARNING_INTERVAL: Duration = Duration::from_secs(60);
const DROP_HOURS: i64 = 14 * 24;
const DROP_RETENTION: SignedDuration = SignedDuration::from_hours(DROP_HOURS);
const DROPPED_LOG_KEYS: usize = 2_000;

/// State shared by the webhook receiver and its single delivery worker.
#[derive(Clone)]
pub struct Webhooks {
    db: Db,
    config: Arc<Config>,
    wake: Arc<Notify>,
    security_log: Arc<Mutex<SecurityLog>>,
    metrics: WebhookMetrics,
}

/// Process-local webhook delivery and listener counters for Settings.
#[derive(Clone, Debug, Default)]
pub struct WebhookMetrics(Arc<Counts>);

#[derive(Debug, Default)]
struct Counts {
    dropped: Mutex<DropCounts>,
    refused_connections: AtomicU32,
}

#[derive(Debug, Default)]
struct DropCounts {
    by_hour: BTreeMap<i64, u32>,
    latest: Option<Timestamp>,
}

fn hour(at: Timestamp) -> i64 {
    at.as_second().div_euclid(3600)
}

fn oldest_bucket(at: Timestamp) -> i64 {
    hour(at) - DROP_HOURS + 1
}

impl WebhookMetrics {
    fn drop_delivery(&self, now: Timestamp) {
        let mut data = self
            .0
            .dropped
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let latest = data.latest.map_or(now, |last| last.max(now));
        if data.latest.is_none_or(|last| hour(latest) != hour(last)) {
            let oldest = oldest_bucket(latest);
            data.by_hour.retain(|bucket, _| *bucket >= oldest);
        }
        data.latest = Some(latest);
        if hour(now) >= oldest_bucket(latest) {
            let count = data.by_hour.entry(hour(now)).or_default();
            *count = count.saturating_add(1);
        }
    }

    pub(crate) async fn status(
        &self,
        db: &Db,
        now: Timestamp,
    ) -> Result<(Option<Timestamp>, u32), db::error::DbError> {
        let last = db.github().last_delivery_at().await?;
        let data = self
            .0
            .dropped
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let count = data
            .by_hour
            .range(oldest_bucket(now)..)
            .fold(0u32, |total, (_, count)| total.saturating_add(*count));
        Ok((
            last.max(data.latest.filter(|last| *last >= now - DROP_RETENTION)),
            count,
        ))
    }

    fn refuse_connection(&self) {
        let _ =
            self.0
                .refused_connections
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                    Some(count.saturating_add(1))
                });
    }

    pub(crate) fn refused_connections(&self) -> u32 {
        self.0.refused_connections.load(Ordering::Relaxed)
    }
}

#[derive(Default)]
struct SecurityLog {
    warnings: [WarningRate; 4],
    installation_cap_warned: bool,
    drop_day: Option<Date>,
    dropped: HashSet<(Option<RepoRef>, Option<i64>)>,
}

#[derive(Clone, Copy)]
enum Warning {
    Signature,
    Target,
    InvalidJson,
    InvalidPayload,
}

#[derive(Default)]
struct WarningRate {
    last: Option<Instant>,
    count: u64,
}

impl WarningRate {
    fn record(&mut self) -> Option<u64> {
        self.count = self.count.saturating_add(1);
        let now = Instant::now();
        if self.last.is_some_and(|last| now - last < WARNING_INTERVAL) {
            return None;
        }
        self.last = Some(now);
        Some(std::mem::take(&mut self.count))
    }
}

impl Webhooks {
    /// Creates the receiver and worker over the given database and configuration.
    pub fn new(db: Db, config: Arc<Config>) -> Self {
        Self {
            db,
            config,
            wake: Arc::default(),
            security_log: Arc::default(),
            metrics: WebhookMetrics::default(),
        }
    }

    /// Shares webhook counters with the public settings route.
    pub fn from_app(state: &AppState) -> Self {
        Self {
            metrics: state.webhook_metrics.clone(),
            ..Self::new(state.db.clone(), state.config.clone())
        }
    }

    /// Starts the single delivery worker and reports whether webhook reception is configured.
    pub fn spawn(&self, state: AppState, stopped: watch::Receiver<bool>) -> Option<JoinHandle<()>> {
        if self.config.github_webhook_secret.is_none() {
            tracing::warn!(
                "GitHub webhook secret not set; repositories are not live, only Refresh and the nightly scan update My work"
            );
            return None;
        }
        Some(tokio::spawn(worker::run(self.clone(), state, stopped)))
    }

    fn warning(&self, warning: Warning) -> Option<u64> {
        let mut log = self
            .security_log
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        log.warnings[warning as usize].record()
    }

    fn first_installation_cap(&self) -> bool {
        let mut log = self
            .security_log
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        !std::mem::replace(&mut log.installation_cap_warned, true)
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
pub async fn serve(
    mut listener: TcpListener,
    app: Router,
    metrics: WebhookMetrics,
    mut stopped: watch::Receiver<bool>,
) {
    let mut connections = JoinSet::new();
    let mut refused = WarningRate::default();
    let slots = Arc::new(Semaphore::new(CONNECTIONS));
    let mut http = hyper::server::conn::http1::Builder::new();
    http.timer(TokioTimer::new())
        .header_read_timeout(HEADER_TIMEOUT);
    loop {
        tokio::select! {
            biased;
            _ = stop(&mut stopped) => break,
            _ = connections.join_next(), if !connections.is_empty() => {}
            (stream, _) = axum::serve::Listener::accept(&mut listener) => {
                let Ok(slot) = Arc::clone(&slots).try_acquire_owned() else {
                    metrics.refuse_connection();
                    if let Some(count) = refused.record() {
                        tracing::warn!(count, limit = CONNECTIONS, "GitHub webhook connection limit reached");
                    }
                    continue;
                };
                let service = TowerToHyperService::new(app.clone());
                let connection = http.serve_connection(TokioIo::new(stream), service);
                let mut stopped = stopped.clone();
                connections.spawn(async move {
                    let _slot = slot;
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
