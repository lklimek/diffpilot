//! Flow file grammar and verification shared by the server and `sr-check` (ADR-014).

mod codec;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::Path;

use contracts::agent::analyse_flow::FlowAnalysisOutput;
use contracts::agent::plan::PlanOutput;
use contracts::ids::{CommitSha, FlowId};
use serde::{Deserialize, Serialize};

use crate::diff::{DiffIndex, FileDiff};
use crate::moves::MovesReport;
use crate::verify::{self, FlowContext, LineFacts};

pub use codec::{encode, parse};

/// Maximum bytes in a flow document (also enforced when collecting the file).
pub const MAX_BYTES: usize = 1024 * 1024;

/// A problem in the model's document. Line numbers are one-based.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {message}")]
pub struct Problem {
    /// Source line, or the document header for cross-document checks.
    pub line: usize,
    /// A repair instruction without internal error details or credentials.
    pub message: String,
    /// Canonical verifier code (grammar problems use schema_mismatch).
    pub kind: verify::VerifyIssueKind,
}

impl Problem {
    fn new(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
            kind: verify::VerifyIssueKind::SchemaMismatch,
        }
    }
}

/// A deterministic location shorthand. Models do not repeat fields already known to the server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    /// Repository-relative path.
    pub file: String,
    /// Serialized LineSide.
    pub side: String,
    /// Inclusive first line.
    pub start: u32,
    /// Inclusive last line.
    pub end: u32,
    /// Serialized FragmentKind.
    pub kind: String,
}

/// Files and planned hunks available in this flow job.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    /// Short file ids, stable in diff order. Extra context files can be declared once in the document.
    pub files: BTreeMap<String, String>,
    /// Planned hunk ids and their deterministic default spans.
    pub hunks: BTreeMap<String, Anchor>,
}

/// No-secret verification context, transported in JobSpec and written beside the checkout.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Context {
    /// Grammar/context version.
    pub version: u32,
    /// Current base commit.
    pub base: CommitSha,
    /// Current head commit.
    pub head: CommitSha,
    /// Whole diff (coverage and moves).
    pub files: Vec<FileDiff>,
    /// Hunks offered to the planner.
    pub listed: Vec<FileDiff>,
    /// Accepted plan.
    pub plan: PlanOutput,
    /// The requested flow.
    pub flow_id: FlowId,
    /// Deterministic move facts.
    pub moves: MovesReport,
    /// Threads actually listed in the prompt.
    pub thread_aliases: Vec<String>,
}

impl Context {
    /// Builds the short-id catalogue from trusted plan/diff facts.
    pub fn catalog(&self) -> Catalog {
        let mut catalog = Catalog::default();
        for file in &self.files {
            for path in std::iter::once(&file.path).chain(file.old_path.iter()) {
                if !catalog.files.values().any(|existing| existing == path) {
                    catalog
                        .files
                        .insert(format!("p{}", catalog.files.len() + 1), path.clone());
                }
            }
        }
        if let Some(flow) = self.plan.flows.iter().find(|flow| flow.id == self.flow_id) {
            for selection in &flow.hunks {
                for file in &self.files {
                    for hunk in &file.hunks {
                        if hunk.id != selection.hunk_id {
                            continue;
                        }
                        let head = hunk.head.len > 0;
                        let span = if head { hunk.head } else { hunk.base };
                        let start = if head {
                            selection.head_start.unwrap_or(span.start)
                        } else {
                            span.start
                        };
                        let end = if head {
                            selection
                                .head_end
                                .unwrap_or(span.start + span.len.saturating_sub(1))
                        } else {
                            span.start + span.len.saturating_sub(1)
                        };
                        let kind = if !head {
                            "deleted_site"
                        } else if hunk.removed.is_empty() {
                            "added"
                        } else {
                            "modified"
                        };
                        catalog.hunks.insert(
                            hunk.id.0.clone(),
                            Anchor {
                                file: if head {
                                    file.path.clone()
                                } else {
                                    file.old_path.clone().unwrap_or_else(|| file.path.clone())
                                },
                                side: if head { "head" } else { "base" }.into(),
                                start,
                                end,
                                kind: kind.into(),
                            },
                        );
                    }
                }
            }
        }
        catalog
    }

    /// Parses and runs the same full flow verifier used by the server, with files read at pinned commits.
    /// The caller supplies the checkout/mirror path; it is never taken from model text.
    pub fn check(&self, text: &str, repo: &Path) -> Result<FlowAnalysisOutput, Vec<Problem>> {
        let fail = |message: String| vec![Problem::new(1, message)];
        if self.version != 1 {
            return Err(fail("unsupported verification context version".into()));
        }
        let output = parse(text, &self.catalog()).map_err(|problem| vec![problem])?;
        let listed = DiffIndex::new(&self.listed).map_err(|e| fail(e.to_string()))?;
        let plan = verify::plan(&self.plan, &listed).map_err(|e| fail(e.to_string()))?;
        let planned = self
            .plan
            .flows
            .iter()
            .find(|f| f.id == self.flow_id)
            .ok_or_else(|| fail("the requested flow is absent from the plan".into()))?;
        let ranges = plan
            .flow_lines
            .get(&self.flow_id)
            .map_or(&[][..], Vec::as_slice);
        let facts = LineFacts::load(
            repo,
            &self.base,
            &self.head,
            &verify::flow_files(&output, ranges),
        )
        .map_err(|_| {
            fail("cannot read the referenced files at the job's base/head commits".into())
        })?;
        let diff = DiffIndex::new(&self.files).map_err(|e| fail(e.to_string()))?;
        let source_lines = codec::source_lines(text).map_err(|problem| vec![problem])?;
        // A field path is never longer than the longest key: longer message prefixes are not tried.
        let longest = source_lines.keys().map(String::len).max().unwrap_or(0);
        verify::flow(
            &output,
            FlowContext {
                planned,
                plan: &plan,
                facts: &facts,
                diff: &diff,
                moves: &self.moves,
                thread_aliases: &self.thread_aliases,
            },
        )
        .map_err(|error| match error {
            verify::VerifyError::Rejected(issues) => issues
                .into_iter()
                .map(|issue| {
                    let line = issue
                        .message
                        .char_indices()
                        .map(|(i, _)| i)
                        .chain(std::iter::once(issue.message.len()))
                        .filter(|end| *end <= longest)
                        .rev()
                        .find_map(|end| source_lines.get(&issue.message[..end]))
                        .copied()
                        .unwrap_or(1);
                    Problem {
                        line,
                        message: issue.message,
                        kind: issue.kind,
                    }
                })
                .collect(),
            _ => fail("the flow could not be verified against the job's files".into()),
        })?;
        Ok(output)
    }
}
