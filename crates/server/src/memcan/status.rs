//! Process-local observations from existing MemCan calls; reading status never probes the service.

use std::sync::{Mutex, PoisonError};

use chrono::Utc;
use contracts::api::settings::{MemcanState, MemcanStatus};
use secrecy::ExposeSecret;

use super::MemcanError;
use crate::config::MemcanConfig;

/// Last completed call and last success; the lock is never held across I/O or an await.
#[derive(Debug)]
pub struct ObservedStatus(Mutex<MemcanStatus>);

impl Default for ObservedStatus {
    fn default() -> Self {
        Self(Mutex::new(MemcanStatus {
            state: MemcanState::NotChecked,
            last_success_at: None,
        }))
    }
}

impl MemcanConfig {
    /// Returns observations since process start; no MemCan call is made.
    pub fn status(&self) -> MemcanStatus {
        if self.token.expose_secret().is_empty() {
            return MemcanStatus {
                state: MemcanState::NotConfigured,
                last_success_at: None,
            };
        }
        self.observed
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn observe<T>(&self, result: &Result<T, &MemcanError>) {
        let mut status = self
            .observed
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        status.state = match result {
            Err(error) if error.is_unavailable() => MemcanState::NotReachable,
            _ => MemcanState::Reachable,
        };
        if result.is_ok() {
            status.last_success_at = Some(Utc::now());
        }
    }
}
