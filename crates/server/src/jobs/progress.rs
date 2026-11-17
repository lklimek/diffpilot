//! In-process progress channel per analysis behind the SSE stream (`docs/contracts/sse-events.md`).
//!
//! Every writer of an analysis or question state goes through [`ProgressHub::update`]: the database write and
//! the publish of its event happen under one lock per analysis, so a subscriber never sees an older state after
//! a newer one.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Utc};
use contracts::api::analyses::{AnalysisState, AnalysisSummary};
use contracts::api::events::AnalysisEvent;
use contracts::ids::AnalysisId;
use db::error::DbError;
use db::repo::Db;
use tokio::sync::broadcast;

/// Events buffered per subscriber; a slower subscriber loses its stream and reconnects (fresh snapshot).
const BUFFER: usize = 64;

/// Per-analysis event channels and update locks; cheap to clone.
#[derive(Clone, Debug, Default)]
pub struct ProgressHub {
    channels: Arc<Mutex<HashMap<AnalysisId, Arc<Channel>>>>,
}

#[derive(Debug)]
struct Channel {
    sender: broadcast::Sender<AnalysisEvent>,
    // Serialises write + publish of every entity of the analysis (a coarser lock than one per entity).
    lock: tokio::sync::Mutex<()>,
}

impl ProgressHub {
    /// Receives every event of `analysis` published from now on (subscribe before reading the snapshot).
    pub fn subscribe(&self, analysis: AnalysisId) -> broadcast::Receiver<AnalysisEvent> {
        self.channel(analysis).sender.subscribe()
    }

    /// Runs `write` (store the change, then read the entity back as an event) under the analysis's lock and
    /// publishes the returned event before releasing it. Nothing is published when `write` fails.
    pub async fn update<F, Fut, E>(
        &self,
        analysis: AnalysisId,
        write: F,
    ) -> Result<AnalysisEvent, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<AnalysisEvent, E>>,
    {
        let channel = self.channel(analysis);
        let _guard = channel.lock.lock().await;
        let event = write().await?;
        // No subscriber is not an error: the next connection reads the stored state.
        let _ = channel.sender.send(event.clone());
        Ok(event)
    }

    /// Stores a new analysis state and publishes the resulting summary.
    pub async fn set_analysis_state(
        &self,
        db: &Db,
        analysis: AnalysisId,
        state: &AnalysisState,
        now: DateTime<Utc>,
    ) -> Result<AnalysisSummary, DbError> {
        let event = self
            .update(analysis, || async {
                db.analyses().set_state(analysis, state, now).await?;
                let summary = db.analyses().summary(analysis).await?;
                Ok::<_, DbError>(AnalysisEvent::Analysis { summary })
            })
            .await?;
        match event {
            AnalysisEvent::Analysis { summary } => Ok(summary),
            AnalysisEvent::Question { .. } | AnalysisEvent::Heartbeat { .. } => {
                Err(DbError::Corrupt {
                    column: "analysis event",
                })
            }
        }
    }

    /// Publishes the analysis's current summary (e.g. its stage changed with a job claim; nothing is written).
    pub async fn publish_analysis(&self, db: &Db, analysis: AnalysisId) -> Result<(), DbError> {
        self.update(analysis, || async {
            let summary = db.analyses().summary(analysis).await?;
            Ok::<_, DbError>(AnalysisEvent::Analysis { summary })
        })
        .await?;
        Ok(())
    }

    fn channel(&self, analysis: AnalysisId) -> Arc<Channel> {
        let mut channels = self.channels.lock().unwrap_or_else(PoisonError::into_inner);
        // Drop channels nobody listens to or updates through; they are recreated on demand.
        channels.retain(|_, channel| {
            channel.sender.receiver_count() > 0 || Arc::strong_count(channel) > 1
        });
        Arc::clone(channels.entry(analysis).or_insert_with(|| {
            Arc::new(Channel {
                sender: broadcast::channel(BUFFER).0,
                lock: tokio::sync::Mutex::new(()),
            })
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::TimeZone;
    use db::repo::analyses::testing::seed_analysis;

    use super::*;

    fn at(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 16, 12, minute, 0)
            .single()
            .unwrap_or_default()
    }

    fn state_of(event: &AnalysisEvent) -> Option<AnalysisState> {
        match event {
            AnalysisEvent::Analysis { summary } => Some(summary.state.clone()),
            AnalysisEvent::Question { .. } | AnalysisEvent::Heartbeat { .. } => None,
        }
    }

    #[tokio::test]
    async fn concurrent_updates_publish_in_write_order_and_last_event_equals_stored_state() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        let hub = ProgressHub::default();
        let mut events = hub.subscribe(analysis);

        // A writes first but publishes slowly; B must not publish before A has published.
        let (a_written, a_written_rx) = tokio::sync::oneshot::channel::<()>();
        let hub_a = hub.clone();
        let db_a = db.clone();
        let a = tokio::spawn(async move {
            hub_a
                .update(analysis, || async {
                    let state = AnalysisState::Running {
                        done: 1,
                        total: 3,
                        stage: None,
                    };
                    db_a.analyses().set_state(analysis, &state, at(1)).await?;
                    let _ = a_written.send(());
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    let summary = db_a.analyses().summary(analysis).await?;
                    Ok::<_, DbError>(AnalysisEvent::Analysis { summary })
                })
                .await
        });
        a_written_rx.await.expect("A wrote");
        let b = hub.set_analysis_state(
            &db,
            analysis,
            &AnalysisState::Running {
                done: 2,
                total: 3,
                stage: None,
            },
            at(2),
        );
        let (a, b) = tokio::join!(a, b);
        a.expect("task").expect("A");
        b.expect("B");

        let first = events.recv().await.expect("first");
        let second = events.recv().await.expect("second");
        assert_eq!(
            state_of(&first),
            Some(AnalysisState::Running {
                done: 1,
                total: 3,
                stage: None
            })
        );
        let stored = db.analyses().get(analysis).await.expect("get").state;
        assert_eq!(state_of(&second), Some(stored));
    }

    #[tokio::test]
    async fn failed_write_publishes_nothing_and_idle_channels_are_dropped() {
        let hub = ProgressHub::default();
        let analysis = AnalysisId::new_random();
        let mut events = hub.subscribe(analysis);
        let failed = hub
            .update(analysis, || async {
                Err::<AnalysisEvent, _>(DbError::NotFound)
            })
            .await;
        assert!(matches!(failed, Err(DbError::NotFound)));
        assert!(matches!(
            events.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        drop(events);
        hub.subscribe(AnalysisId::new_random());
        let channels = hub.channels.lock().expect("lock");
        assert!(!channels.contains_key(&analysis));
    }
}
