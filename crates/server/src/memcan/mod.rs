//! MemCan: feedback stored as memories and severity calibration examples read back for the prompts (brief §5
//! Calibration memory; `project` = the analysed repository's short name). MemCan is optional at run time: when
//! it is unreachable, feedback stays stored locally and queued; nothing is reported as saved to MemCan unless
//! MemCan confirmed it.
//!
//! Leaves the server (AR-1, threat model T6), per feedback entry, as one memory keyed by the feedback id: the
//! repository `owner/name`, pull request number and title, the value, the reviewer's note, the target (step
//! title, file and lines, the LLM's risk band and headline; annotation type, lines and text; or path, side and
//! line), and the opaque feedback id and revision. Never the user's identity, tokens or other analysis content.

pub mod calibration;
mod client;
pub mod feedback;
mod memory;
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

pub use client::MemcanError;

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
