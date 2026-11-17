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

/// The project profile memory of `project` (the analysed repository's short name). An error means "not checked",
/// never "no profile".
pub async fn project_profile(
    config: &MemcanConfig,
    project: &str,
) -> Result<ProfileLookup, MemcanError> {
    const TOOL: &str = "search_memories";
    let client = MemcanClient::connect(config).await?;
    let answer = client
        .call(
            TOOL,
            json!({"query": QUERY, "project": project, "limit": SEARCH_LIMIT}),
        )
        .await?;
    let lookup = parse_profile(&answer).ok_or(MemcanError::InvalidResponse { tool: TOOL })?;
    if lookup == ProfileLookup::NotChecked {
        tracing::warn!(
            project,
            "MemCan's bounded search held no project profile but not every memory; the profile is not checked"
        );
    }
    Ok(lookup)
}

/// The stored lookup of `analysis`; [`ProfileLookup::NotChecked`] when none is stored.
pub async fn stored(db: &Db, analysis: AnalysisId) -> Result<ProfileLookup, DbError> {
    Ok(db
        .documents()
        .get(analysis, DocumentKind::MemcanProfile, "")
        .await?
        .unwrap_or(ProfileLookup::NotChecked))
}

/// The analysis's lookup and whether MemCan failed in it just now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ensured {
    /// The lookup.
    pub lookup: ProfileLookup,
    /// MemCan was called now and failed: the run treats it as down.
    pub memcan_failed: bool,
}

/// The stored lookup of the analysis, made and stored first when missing (once per round: every prompt of the
/// analysis gets the same profile).
pub async fn ensure(
    db: &Db,
    config: &MemcanConfig,
    summary: &AnalysisSummary,
) -> Result<Ensured, DbError> {
    if let Some(lookup) = db
        .documents()
        .get(summary.id, DocumentKind::MemcanProfile, "")
        .await?
    {
        return Ok(Ensured {
            lookup,
            memcan_failed: false,
        });
    }
    let mut memcan_failed = false;
    let lookup = match project_profile(config, &summary.pr.repo.name).await {
        Ok(lookup) => lookup,
        Err(error) => {
            memcan_failed = true;
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
    Ok(Ensured {
        lookup,
        memcan_failed,
    })
}

/// Before an explicit Retry: forgets a `not_checked` lookup so the new round looks the profile up again; a
/// `checked` one is kept for the analysis.
pub async fn forget_not_checked(db: &Db, analysis: AnalysisId) -> Result<(), DbError> {
    if stored(db, analysis).await? == ProfileLookup::NotChecked {
        db.documents()
            .delete(analysis, DocumentKind::MemcanProfile, "")
            .await?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct SearchAnswer {
    memories: Vec<serde_json::Value>,
    has_more: Option<bool>,
}

#[derive(Deserialize)]
struct Memory {
    memory: String,
    #[serde(rename = "type")]
    kind: String,
}

/// The lookup a `search_memories` answer proves; `None` if the answer is not one. MemCan has no search by type, so
/// "no profile" holds only when the answer held every memory of the project (`has_more` false, fewer than the
/// limit); otherwise a profile may rank below the limit and the lookup is not checked.
fn parse_profile(answer: &str) -> Option<ProfileLookup> {
    let answer: SearchAnswer = serde_json::from_str(answer).ok()?;
    let exhaustive =
        answer.has_more == Some(false) && answer.memories.len() < SEARCH_LIMIT as usize;
    let text = answer
        .memories
        .into_iter()
        .filter_map(|memory| serde_json::from_value::<Memory>(memory).ok())
        .filter(|memory| memory.kind == PROFILE_TYPE)
        .map(|memory| bounded(&memory.memory))
        .find(|text| !text.is_empty());
    Some(match text {
        Some(text) => ProfileLookup::Checked { text: Some(text) },
        None if exhaustive => ProfileLookup::Checked { text: None },
        None => ProfileLookup::NotChecked,
    })
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
