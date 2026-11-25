//! The reader's daily dashboard ranking usage and reset (C8).

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use contracts::api::settings::RankingUsage;
use contracts::routes::{GET_RANKING_USAGE, RESET_RANKING_USAGE};
use contracts::time::Timestamp;
use db::error::DbError;
use jiff::civil::Time;

use super::error::ApiFailure;
use crate::auth::{Session, SessionCsrf};
use crate::dashboard::judge::{DAILY_CAP, REFRESH_CAP, capped_count, day_start, ranking_zone};
use crate::state::AppState;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(GET_RANKING_USAGE.path, get(usage))
        .route(RESET_RANKING_USAGE.path, post(reset))
}

async fn usage(
    State(state): State<AppState>,
    session: Session,
) -> Result<Json<RankingUsage>, ApiFailure<DbError>> {
    Ok(Json(usage_for(&state, &session, false).await?))
}

async fn reset(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
) -> Result<Json<RankingUsage>, ApiFailure<DbError>> {
    Ok(Json(usage_for(&state, &session, true).await?))
}

async fn usage_for(
    state: &AppState,
    session: &Session,
    reset: bool,
) -> Result<RankingUsage, DbError> {
    let tenant = &session.tenant_id;
    let user = &session.user_id;
    let zone = ranking_zone(state, tenant, user).await?;
    let now = state.dashboard.now();
    let start = day_start(now, &zone);
    let resets_at = now
        .to_zoned(zone.clone())
        .date()
        .tomorrow()
        .and_then(|day| day.to_datetime(Time::midnight()).to_zoned(zone))
        .map(|day| Timestamp::from(day.timestamp()))
        .map_err(|_| DbError::Corrupt {
            column: "dashboard_settings.time_zone",
        })?;
    let dashboard = state.db.dashboard();
    if reset {
        let sent_before = dashboard
            .reset_ranking_usage(tenant, user, start, now)
            .await?;
        tracing::info!(tenant_id = %tenant, user_id = %user, sent_before,
            "dashboard ranking usage reset");
    }
    Ok(RankingUsage {
        sent_today: dashboard.sent_today(tenant, user, start).await?,
        daily_cap: DAILY_CAP,
        per_refresh_cap: REFRESH_CAP,
        resets_at,
        left_provisional: capped_count(state, tenant, user).await?,
    })
}
