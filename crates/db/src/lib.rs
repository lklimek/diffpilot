//! The database module: SQLite schema and repository API. The only crate that uses sqlx; its public API exposes
//! no sqlx types.

pub mod error;
pub mod repo;
