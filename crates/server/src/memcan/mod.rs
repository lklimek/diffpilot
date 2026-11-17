//! MemCan: feedback stored as memories, severity calibration examples and the project profile read back for the
//! prompts (brief §5 Calibration memory, §2 Project profile; `project` = the analysed repository's short name).
//! MemCan is optional at run time: when it is unreachable, feedback stays stored locally and queued; nothing is
//! reported as saved to MemCan unless MemCan confirmed it.
//!
//! Leaves the server (AR-1, threat model T6), per feedback entry, as one memory keyed by the feedback id: the
//! repository `owner/name`, pull request number and title, the fixed value, the public target (step
//! title, file and lines, the LLM's risk band and headline; annotation type, lines and text; or path, side and
//! line), and the opaque feedback id and revision. Never private notes, the user's identity, tokens or other analysis content.

pub mod calibration;
mod client;
pub mod feedback;
mod memory;
pub mod profile;
pub mod status;
pub mod writer;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;

use contracts::agent::analyse_flow::FlowAnalysisOutput;
use contracts::ids::AnalysisId;
use db::error::DbError;
use db::repo::Db;
use db::repo::documents::DocumentKind;
use tracing_subscriber::filter::{LevelFilter, Targets};

pub use client::MemcanError;

/// Caps the log targets that could print MCP or HTTP payloads (MemCan text, a token echoed by a hostile server,
/// request URLs) regardless of `RUST_LOG`: rmcp logs nothing (its errors reach the server's own scrubbed log
/// lines), the HTTP stack at most `INFO`. Applied by the server's and the tests' log setup.
pub fn payload_log_cap() -> Targets {
    Targets::new()
        .with_default(LevelFilter::TRACE)
        .with_target("rmcp", LevelFilter::OFF)
        .with_target("reqwest", LevelFilter::INFO)
        .with_target("hyper", LevelFilter::INFO)
        .with_target("hyper_util", LevelFilter::INFO)
        .with_target("h2", LevelFilter::INFO)
}

/// The stored flow analyses of an analysis.
async fn stored_flows(db: &Db, analysis: AnalysisId) -> Result<Vec<FlowAnalysisOutput>, DbError> {
    Ok(db
        .documents()
        .list(analysis, DocumentKind::Flow)
        .await?
        .into_iter()
        .map(|(_, flow)| flow)
        .collect())
}
