//! GitHub work dashboard ("My work", ADR-016, ADR-017): refresh orchestration, scoring, judgement, per-user
//! settings and the daily schedule. Reads the review module only through the database.

pub mod hub;
pub mod judge;
pub mod refresh;
pub mod schedule;
pub mod score;
pub mod settings;
pub mod snapshot;

pub use hub::DashboardHub;
