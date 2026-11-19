//! The database module: SQLite schema and repository API. The only crate that uses sqlx; its public API exposes
//! no sqlx types. Transactions start only through `repo::write_tx` (`BEGIN IMMEDIATE`, for anything that writes)
//! or `repo::read_tx` (read-only); clippy (`clippy.toml`) rejects any other way to begin one.

pub mod error;
pub mod repo;
