//! The MemCan project profile (brief §2 Project profile): the memory of type `project-profile` in the analysed
//! repository's project, looked up once per analysis and stored with its documents. The memory is untrusted data
//! (any MemCan writer, the agent included, can write one): only its text is kept, without control characters
//! other than line breaks and tabs, cut to [`MAX_PROFILE_CHARS`]; the prompts quote it as data.

use contracts::api::analyses::AnalysisSummary;
use contracts::ids::AnalysisId;
use db::error::DbError;
use db::repo::Db;
use db::repo::documents::DocumentKind;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::client::{MemcanClient, MemcanError};
use crate::config::MemcanConfig;

/// Longest profile text kept, in characters.
pub const MAX_PROFILE_CHARS: usize = 16_000;
/// Metadata `type` of a project profile memory.
const PROFILE_TYPE: &str = "project-profile";
/// Search query sent.
const QUERY: &str = "project profile";
/// Memories asked for; other types are dropped.
const SEARCH_LIMIT: u32 = 10;

/// The outcome of the analysis's MemCan profile lookup.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "memcan", rename_all = "snake_case")]
pub enum ProfileLookup {
    /// MemCan was not searched (unreachable or failed): a profile may exist.
    NotChecked,
    /// MemCan was searched; `text` is the profile, `None` when there is none.
    Checked {
        /// The bounded, untrusted profile text.
        text: Option<String>,
    },
}

impl ProfileLookup {
    /// `None` when not checked, else whether a profile was found (`Prepass::profile_status`).
    pub fn found(&self) -> Option<bool> {
        match self {
            Self::NotChecked => None,
            Self::Checked { text } => Some(text.is_some()),
        }
    }

    /// The profile text, if one was found.
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Checked { text: Some(text) } => Some(text),
            _ => None,
        }
    }
}

/// The project profile memory of `project` (the analysed repository's short name); `Ok(None)` when MemCan has
/// none. An error means "not checked", never "no profile".
pub async fn project_profile(
    config: &MemcanConfig,
    project: &str,
) -> Result<Option<String>, MemcanError> {
    const TOOL: &str = "search_memories";
    let client = MemcanClient::connect(config).await?;
    let answer = client
        .call(
            TOOL,
            json!({"query": QUERY, "project": project, "limit": SEARCH_LIMIT}),
        )
        .await?;
    parse_profile(&answer).ok_or(MemcanError::InvalidResponse { tool: TOOL })
}

/// The stored lookup of `analysis`; [`ProfileLookup::NotChecked`] when none is stored.
pub async fn stored(db: &Db, analysis: AnalysisId) -> Result<ProfileLookup, DbError> {
    Ok(db
        .documents()
        .get(analysis, DocumentKind::MemcanProfile, "")
        .await?
        .unwrap_or(ProfileLookup::NotChecked))
}

/// The stored lookup of the analysis, made and stored first when missing (once per analysis: every prompt of
/// the analysis gets the same profile).
pub async fn ensure(
    db: &Db,
    config: &MemcanConfig,
    summary: &AnalysisSummary,
) -> Result<ProfileLookup, DbError> {
    if let Some(lookup) = db
        .documents()
        .get(summary.id, DocumentKind::MemcanProfile, "")
        .await?
    {
        return Ok(lookup);
    }
    let lookup = match project_profile(config, &summary.pr.repo.name).await {
        Ok(text) => ProfileLookup::Checked { text },
        Err(error) => {
            tracing::warn!(analysis = %summary.id, %error,
                "MemCan project profile not readable; the analysis runs without it (not checked)");
            ProfileLookup::NotChecked
        }
    };
    db.documents()
        .put(
            summary.id,
            DocumentKind::MemcanProfile,
            "",
            &lookup,
            chrono::Utc::now(),
        )
        .await?;
    Ok(lookup)
}

#[derive(Deserialize)]
struct SearchAnswer {
    memories: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct Memory {
    memory: String,
    #[serde(rename = "type")]
    kind: String,
}

/// The first project profile in a `search_memories` answer (`Some(None)`: none); `None` if the answer is not one.
fn parse_profile(answer: &str) -> Option<Option<String>> {
    let answer: SearchAnswer = serde_json::from_str(answer).ok()?;
    Some(
        answer
            .memories
            .into_iter()
            .filter_map(|memory| serde_json::from_value::<Memory>(memory).ok())
            .filter(|memory| memory.kind == PROFILE_TYPE)
            .map(|memory| bounded(&memory.memory))
            .find(|text| !text.is_empty()),
    )
}

/// `text` without control characters other than line breaks and tabs, trimmed and cut to [`MAX_PROFILE_CHARS`].
fn bounded(text: &str) -> String {
    let kept: String = text
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .take(MAX_PROFILE_CHARS)
        .collect();
    kept.trim().to_owned()
}
