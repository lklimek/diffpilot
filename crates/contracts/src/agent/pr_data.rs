//! Host-supplied review inputs written as separate files under /work/pr-data.

use super::comment_report::{RecordedDecisionKind, SourceKind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use ts_rs::TS;

/// Self-contained host-supplied review files; GitHub shapes are opaque to contracts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PrData {
    /// manifest.json.
    pub manifest: Manifest,
    /// threads.json in the gh script shape.
    #[ts(type = "unknown")]
    pub threads: Value,
    /// review-comments.json in the gh script shape.
    #[ts(type = "unknown[]")]
    pub review_comments: Vec<Value>,
    /// reviews.json in the gh script shape.
    #[ts(type = "unknown[]")]
    pub reviews: Vec<Value>,
    /// comments.json in the gh script shape.
    #[ts(type = "unknown[]")]
    pub comments: Vec<Value>,
    /// Base-branch convention files, relative paths below conventions/.
    pub conventions: BTreeMap<String, String>,
    /// ids.json, preserving durable point identities and standing decisions.
    pub ids: ItemIds,
    /// scope.json; all other handed-in material is context.
    pub scope: ReviewScope,
}

/// Host provenance and author facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Manifest {
    /// owner/repo.
    pub repo: String,
    /// PR number.
    pub pr: u32,
    /// Assessed head.
    pub head: String,
    /// Base branch name.
    pub base_ref: String,
    /// Frozen base commit for pre-existing checks.
    pub base_sha: String,
    /// Author-stated PR intent.
    pub title: String,
    /// Author-stated PR description.
    pub body: String,
    /// Trusted maintainer relays.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delegates: Vec<String>,
    /// PR author's login; only their own text is author context.
    pub pr_author: String,
    /// Tenant members' logins, independent of source roles.
    pub maintainers: Vec<String>,
    /// Known bot logins.
    pub bots: Vec<String>,
}

/// Durable CMT number allocation and previous item identities.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ItemIds {
    /// Next unused number.
    pub next_id: u32,
    /// Known point identities.
    pub items: Vec<PrDataItem>,
}

/// One stable point in ids.json.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PrDataItem {
    /// CMT identifier.
    pub id: String,
    /// Thread node id, review:id:k or comment:id:k.
    pub key: String,
    /// Original source kind, inferred from the key when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<SourceKind>,
    /// Quote only for open and decided items.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    /// Standing maintainer decision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<StandingDecision>,
}

/// Standing choice offered to the skill, without invented reply-chain provenance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct StandingDecision {
    /// Stored decision kind.
    pub kind: RecordedDecisionKind,
    /// Existing PR target.
    pub target_pr: Option<u32>,
    /// Existing issue or PR target.
    pub target_url: Option<String>,
    /// Follow-up or TODO title.
    pub title: Option<String>,
}

/// What a pass may assess (10 points/threads and 5 new sources, with continuations).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReviewScope {
    /// Open thread node ids.
    pub threads: Vec<String>,
    /// Versioned source URLs.
    pub sources: Vec<String>,
}
