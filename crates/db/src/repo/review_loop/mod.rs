//! Repository for the review loop (ADR-023, migration 0017): PR read state, items with revisions, assessments and
//! passes, append-only decisions with their effects, claims and leases, the carry-over log, watches, per-user
//! events, notification channels, agent subscriptions and deliveries. Secrets arrive and leave as ciphertext only.

use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqlitePool};

use contracts::time::Timestamp;

use super::Db;
use super::jobs::{parse_ts, parse_wire, wire_name};
use crate::error::DbError;

mod actions;
mod carry;
mod decisions;
mod items;
mod notify;
mod watches;

pub use actions::ActionOutcome;
pub use carry::NewCarryEvent;
pub use decisions::{
    ClaimOutcome, DecideOutcome, NewDecision, NewEffect, ReportOutcomeRecord, ReportRequest,
    StoredDecision,
};
pub use items::{
    CommentPrRecord, ItemRecord, NewPass, ObservedItem, PassCause, PassRecord, PassStart,
    PassState, ReadResult, SentItem, StoredAssessment, UpsertOutcome,
};
pub use notify::{
    ChannelInput, ChannelRecord, DeliveryRecord, DeliveryTarget, FailureTransition, PushInput,
    SecretUpdate, SubscriptionRecord,
};
pub use watches::{NewUserEvent, WatchOutcome, WatchRecord};

#[cfg(test)]
mod tests;

/// Access to the review-loop tables.
#[derive(Debug)]
pub struct ReviewLoop<'a> {
    pool: &'a SqlitePool,
}

impl Db {
    /// The review loop (ADR-023).
    pub fn review_loop(&self) -> ReviewLoop<'_> {
        ReviewLoop { pool: &self.pool }
    }
}

/// JSON text of a stored document.
fn json<T: Serialize>(value: &T) -> Result<String, DbError> {
    Ok(serde_json::to_string(value)?)
}

/// A JSON column.
fn json_col<T: DeserializeOwned>(row: &SqliteRow, column: &'static str) -> Result<T, DbError> {
    let text: String = row.try_get(column)?;
    serde_json::from_str(&text).map_err(|_| DbError::Corrupt { column })
}

/// A nullable JSON column.
fn opt_json_col<T: DeserializeOwned>(
    row: &SqliteRow,
    column: &'static str,
) -> Result<Option<T>, DbError> {
    let text: Option<String> = row.try_get(column)?;
    text.map(|text| serde_json::from_str(&text).map_err(|_| DbError::Corrupt { column }))
        .transpose()
}

/// A text column holding a serde name.
fn wire_col<T: DeserializeOwned>(row: &SqliteRow, column: &'static str) -> Result<T, DbError> {
    let text: String = row.try_get(column)?;
    parse_wire(&text, column)
}

/// A nullable text column holding a serde name.
fn opt_wire_col<T: DeserializeOwned>(
    row: &SqliteRow,
    column: &'static str,
) -> Result<Option<T>, DbError> {
    let text: Option<String> = row.try_get(column)?;
    text.map(|text| parse_wire(&text, column)).transpose()
}

/// A timestamp column.
fn ts_col(row: &SqliteRow, column: &'static str) -> Result<Timestamp, DbError> {
    let text: String = row.try_get(column)?;
    parse_ts(&text, column)
}

/// A nullable timestamp column.
fn opt_ts_col(row: &SqliteRow, column: &'static str) -> Result<Option<Timestamp>, DbError> {
    let text: Option<String> = row.try_get(column)?;
    text.map(|text| parse_ts(&text, column)).transpose()
}

/// A non-negative integer column as `u32`.
fn u32_col(row: &SqliteRow, column: &'static str) -> Result<u32, DbError> {
    let value: i64 = row.try_get(column)?;
    u32::try_from(value).map_err(|_| DbError::Corrupt { column })
}

/// The serde name of a closed enum, for a CHECK-listed column.
fn wire<T: Serialize>(value: T) -> Result<String, DbError> {
    wire_name(value)
}
